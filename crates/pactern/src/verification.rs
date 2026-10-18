//! External SMT verification for the finite, loop-free pure core fragment.
//!
//! Checked integers use 64-bit vectors, with explicit widened arithmetic safety
//! conditions. Solver absence/timeouts never become proof. No bounded search is
//! reported as an unbounded theorem.
use crate::{
    core::*,
    resolution::{SymbolId, SymbolKind},
    syntax::{BinaryOp, UnaryOp},
    typing::ValueType,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Proved,
    Refuted,
    Unknown,
    Unsupported,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub status: Status,
    pub reason: String,
    pub counterexample: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub program_hash: String,
    pub function: usize,
    pub precondition_consistency: Claim,
    pub postcondition: Claim,
    pub absence_of_traps: Claim,
    pub termination: Claim,
    pub solver: String,
    pub assumptions: Vec<String>,
    /// Always None: this verifier does not perform bounded exploration.
    pub exploration_bound: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct Solver {
    pub executable: String,
    pub timeout_ms: u64,
}
impl Default for Solver {
    fn default() -> Self {
        Self {
            executable: "z3".into(),
            timeout_ms: 5_000,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Queries {
    pub precondition_consistency: String,
    pub postcondition: String,
    pub absence_of_traps: String,
}
fn claim(status: Status, reason: impl Into<String>) -> Claim {
    Claim {
        status,
        reason: reason.into(),
        counterexample: None,
    }
}

/// Compiler-version-scoped fingerprint of the complete checked program.
/// This prevents pairing reports from unrelated programs that reuse local IDs.
pub fn program_fingerprint(program: &Program) -> String {
    use sha2::{Digest, Sha256};
    struct Hasher(Sha256);
    impl std::fmt::Write for Hasher {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0.update(text.as_bytes());
            Ok(())
        }
    }
    let mut hasher = Hasher(Sha256::new());
    use std::fmt::Write;
    write!(
        &mut hasher,
        "pactern/core-evidence/1\n{}\n{program:?}",
        crate::TOOL_VERSION
    )
    .expect("hash formatter cannot fail");
    format!("sha256:{:x}", hasher.0.finalize())
}

/// Generate reviewable SMT-LIB queries. Unsupported programs return a reason.
pub fn queries(program: &Program, function: SymbolId) -> Result<Queries, String> {
    let mut encoder = Encoder::new(program);
    let declaration = encoder.function(function)?;
    let DeclarationKind::Function {
        type_parameters,
        parameters,
        result,
        effects,
        ..
    } = &declaration.kind
    else {
        unreachable!()
    };
    if !type_parameters.is_empty() || effects.as_ref().is_some_and(|e| !e.is_empty()) {
        return Err("generic/effectful targets are unsupported".into());
    }
    encoder.sort(result)?;
    let mut inputs = Vec::new();
    let mut env = BTreeMap::new();
    for parameter in parameters {
        let ty = program
            .symbol_types()
            .get(parameter)
            .ok_or("missing parameter type")?
            .clone();
        let sort = encoder.sort(&ty)?;
        let term = format!("arg_{}", parameter.0);
        inputs.push(format!("(declare-const {term} {sort})"));
        env.insert(
            *parameter,
            Sym {
                term,
                ty,
                safe: "true".into(),
            },
        );
    }
    let (pre, pre_safe) = encoder.clauses(function, true, &env)?;
    let args = parameters.iter().map(|id| env[id].clone()).collect();
    let value = encoder.invoke(function, args, false)?;
    for symbol in &program.resolution().symbols {
        if symbol.kind == SymbolKind::Result && symbol.parent == Some(function) {
            env.insert(symbol.id, value.clone());
        }
    }
    let (post, post_safe) = encoder.clauses(function, false, &env)?;
    let base = format!(
        "(set-logic ALL)\n(set-option :produce-models true)\n{}\n{}\n",
        encoder.declarations.join("\n"),
        inputs.join("\n")
    );
    let query = |condition: String| format!("{base}(assert {condition})\n(check-sat)\n");
    Ok(Queries {
        precondition_consistency: query(and([pre_safe.clone(), pre.clone()])),
        postcondition: query(and([
            pre_safe.clone(),
            pre.clone(),
            value.safe.clone(),
            post_safe.clone(),
            not(&post),
        ])),
        absence_of_traps: query(or([
            not(&pre_safe),
            and([pre_safe, pre, not(&and([value.safe, post_safe]))]),
        ])),
    })
}
/// Invoke an external solver, separately checking all three logical claims.
pub fn verify(program: &Program, function: SymbolId, solver: &Solver) -> Report {
    let version = run_solver_process(&solver.executable, &["-version"], "", solver.timeout_ms)
        .map(|text| text.trim().to_owned())
        .unwrap_or_else(|_| "unavailable".into());
    let mut report = Report {
        program_hash: program_fingerprint(program),
        function: function.0,
        precondition_consistency: claim(Status::Unknown, "not checked"),
        postcondition: claim(Status::Unknown, "not checked"),
        absence_of_traps: claim(Status::Unknown, "not checked"),
        termination: claim(Status::Unknown, "not checked"),
        solver: version,
        assumptions: vec![
            "Checked core IR and the C/Wasm translation must preserve language semantics".into(),
            "Termination assumes sufficient fuel, memory, and stack; host failures are excluded"
                .into(),
            "External solver answers are trusted; no proof certificate is checked".into(),
        ],
        exploration_bound: None,
    };
    let q = match queries(program, function) {
        Ok(q) => q,
        Err(reason) => {
            report.precondition_consistency = claim(Status::Unsupported, &reason);
            report.postcondition = claim(Status::Unsupported, &reason);
            report.absence_of_traps = claim(Status::Unsupported, &reason);
            report.termination = claim(Status::Unsupported, reason);
            return report;
        }
    };
    report.precondition_consistency = solve(solver, &q.precondition_consistency, true);
    report.termination = claim(
        Status::Proved,
        "finite loop-free body and acyclic inlined calls; sufficient execution resources assumed",
    );
    if report.precondition_consistency.status == Status::Refuted {
        report.postcondition = claim(
            Status::Unknown,
            "inconsistent preconditions reject vacuous proof",
        );
        report.absence_of_traps = claim(
            Status::Unknown,
            "inconsistent preconditions reject vacuous proof",
        );
    } else {
        report.postcondition = solve(solver, &q.postcondition, false);
        report.absence_of_traps = solve(solver, &q.absence_of_traps, false);
    }
    report
}
fn solve(solver: &Solver, query: &str, satisfiable_is_success: bool) -> Claim {
    let script = format!(
        "(set-option :timeout {})\n{query}",
        solver.timeout_ms.max(1)
    );
    let answer = match run_solver_process(
        &solver.executable,
        &["-in", "-smt2"],
        &script,
        solver.timeout_ms,
    ) {
        Ok(answer) => answer,
        Err(reason) => return claim(Status::Unknown, reason),
    };
    let first = answer.lines().next().unwrap_or("");
    if !matches!(first, "sat" | "unsat") {
        return claim(
            Status::Unknown,
            format!("solver returned {}", answer.trim()),
        );
    }
    let success = (first == "sat") == satisfiable_is_success;
    let mut result = claim(
        if success {
            Status::Proved
        } else {
            Status::Refuted
        },
        if satisfiable_is_success {
            "SMT precondition satisfiability"
        } else {
            "unbounded SMT counterexample query"
        },
    );
    if first == "sat"
        && !success
        && let Ok(model) = run_solver_process(
            &solver.executable,
            &["-in", "-smt2"],
            &format!("{script}(get-model)\n"),
            solver.timeout_ms,
        )
        && model.lines().next() == Some("sat")
    {
        result.counterexample = Some(model.chars().take(16_384).collect());
    }
    result
}
fn run_solver_process(
    executable: &str,
    args: &[&str],
    input: &str,
    timeout_ms: u64,
) -> Result<String, String> {
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("solver unavailable: {e}"))?;
    let stdout = child.stdout.take().ok_or("solver output unavailable")?;
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(1_048_576)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let mut stdin = child.stdin.take().ok_or("solver input unavailable")?;
    let input = input.as_bytes().to_vec();
    let writer = thread::spawn(move || stdin.write_all(&input));
    let deadline =
        Instant::now() + Duration::from_millis(timeout_ms.saturating_add(100).clamp(100, 120_000));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                let _ = writer.join();
                return Err("solver timed out or process failed".into());
            }
        }
    };
    writer
        .join()
        .map_err(|_| "solver input thread failed")?
        .map_err(|e| e.to_string())?;
    let output = reader
        .join()
        .map_err(|_| "solver output thread failed")?
        .map_err(|e| e.to_string())?;
    if output.len() == 1_048_576 {
        return Err("solver output size limit exceeded".into());
    }
    if !status.success() {
        return Err(format!(
            "solver process failed: {}",
            String::from_utf8_lossy(&output)
        ));
    }
    String::from_utf8(output).map_err(|_| "solver output was not UTF-8".into())
}

#[derive(Clone)]
struct Sym {
    term: String,
    ty: ValueType,
    safe: String,
}
#[derive(Clone)]
struct State {
    path: String,
    env: BTreeMap<SymbolId, Sym>,
    value: Option<Sym>,
}
struct Encoder<'a> {
    program: &'a Program,
    declarations: Vec<String>,
    sorts: BTreeMap<String, String>,
    building: BTreeSet<String>,
    active: Vec<SymbolId>,
    nodes: usize,
}
fn and<const N: usize>(parts: [String; N]) -> String {
    conjunction(parts.into_iter().collect())
}
fn conjunction(parts: Vec<String>) -> String {
    if parts.is_empty() {
        "true".into()
    } else {
        format!("(and {})", parts.join(" "))
    }
}
fn or<const N: usize>(parts: [String; N]) -> String {
    format!("(or {})", parts.join(" "))
}
fn not(a: &str) -> String {
    format!("(not {a})")
}
fn ite(c: &str, a: &str, b: &str) -> String {
    format!("(ite {c} {a} {b})")
}
fn bv(value: i64) -> String {
    format!("(_ bv{} 64)", value as u64)
}
impl<'a> Encoder<'a> {
    fn new(program: &'a Program) -> Self {
        Self {
            program,
            declarations: vec![],
            sorts: BTreeMap::new(),
            building: BTreeSet::new(),
            active: vec![],
            nodes: 0,
        }
    }
    fn budget(&mut self) -> Result<(), String> {
        self.nodes += 1;
        if self.nodes > 10_000 {
            Err("symbolic expansion limit exceeded".into())
        } else {
            Ok(())
        }
    }
    fn function(&self, id: SymbolId) -> Result<&'a Declaration, String> {
        self.program
            .declarations()
            .iter()
            .find(|d| d.symbol == id && matches!(d.kind, DeclarationKind::Function { .. }))
            .ok_or_else(|| "unknown function".into())
    }
    fn variants(&self, ty: &ValueType) -> Result<Vec<(SymbolId, Vec<ValueType>)>, String> {
        let builtin = |name: &str| {
            self.program
                .resolution()
                .symbols
                .iter()
                .find(|s| s.kind == SymbolKind::BuiltinVariant && s.name == name)
                .map(|s| s.id)
                .ok_or_else(|| "missing builtin variant".to_string())
        };
        match ty {
            ValueType::Option(inner) => Ok(vec![
                (builtin("None")?, vec![]),
                (builtin("Some")?, vec![*inner.clone()]),
            ]),
            ValueType::Result(ok, err) => Ok(vec![
                (builtin("Ok")?, vec![*ok.clone()]),
                (builtin("Err")?, vec![*err.clone()]),
            ]),
            ValueType::Named(id) | ValueType::Applied(id, _) => {
                let declaration = self
                    .program
                    .declarations()
                    .iter()
                    .find(|d| d.symbol == *id)
                    .ok_or("unknown datatype")?;
                let DeclarationKind::Enum {
                    type_parameters,
                    variants,
                } = &declaration.kind
                else {
                    return Err("not an enum".into());
                };
                let args = if let ValueType::Applied(_, a) = ty {
                    a.clone()
                } else {
                    vec![]
                };
                let subs = type_parameters.iter().copied().zip(args).collect();
                Ok(variants
                    .iter()
                    .map(|v| {
                        (
                            v.symbol,
                            v.payload
                                .iter()
                                .map(|ty| crate::core::substitute(ty, &subs))
                                .collect(),
                        )
                    })
                    .collect())
            }
            _ => Err("not an enum".into()),
        }
    }
    fn fields(&self, ty: &ValueType) -> Option<Vec<(SymbolId, ValueType)>> {
        let (id, args) = match ty {
            ValueType::Named(id) => (id, vec![]),
            ValueType::Applied(id, args) => (id, args.clone()),
            _ => return None,
        };
        let declaration = self
            .program
            .declarations()
            .iter()
            .find(|d| d.symbol == *id)?;
        let DeclarationKind::Struct {
            type_parameters,
            fields,
        } = &declaration.kind
        else {
            return None;
        };
        let subs = type_parameters.iter().copied().zip(args).collect();
        Some(
            fields
                .iter()
                .map(|id| {
                    (
                        *id,
                        crate::core::substitute(&self.program.symbol_types()[id], &subs),
                    )
                })
                .collect(),
        )
    }
    fn sort(&mut self, ty: &ValueType) -> Result<String, String> {
        match ty {
            ValueType::I64 => return Ok("(_ BitVec 64)".into()),
            ValueType::Bool => return Ok("Bool".into()),
            _ => {}
        }
        let key = format!("{ty:?}");
        if self.building.contains(&key) {
            return Err("recursive datatypes are unsupported".into());
        }
        if let Some(sort) = self.sorts.get(&key) {
            return Ok(sort.clone());
        }
        if matches!(
            ty,
            ValueType::String | ValueType::List(_) | ValueType::Parameter(_)
        ) {
            return Err("strings, lists and polymorphic values are unsupported".into());
        }
        let name = format!("D{}", self.sorts.len());
        self.sorts.insert(key.clone(), name.clone());
        self.building.insert(key.clone());
        let constructors = if *ty == ValueType::Unit {
            format!("({name}_unit)")
        } else if let Some(fields) = self.fields(ty) {
            let mut selectors = Vec::new();
            for (id, ty) in fields {
                selectors.push(format!("({name}_f{} {})", id.0, self.sort(&ty)?));
            }
            format!("({name}_record {})", selectors.join(" "))
        } else {
            let mut constructors = Vec::new();
            for (id, payload) in self.variants(ty)? {
                let mut selectors = Vec::new();
                for (i, ty) in payload.iter().enumerate() {
                    selectors.push(format!("({name}_v{}_p{i} {})", id.0, self.sort(ty)?));
                }
                constructors.push(format!("({name}_v{} {})", id.0, selectors.join(" ")));
            }
            constructors.join(" ")
        };
        self.declarations
            .push(format!("(declare-datatypes () (({name} {constructors})))"));
        self.building.remove(&key);
        Ok(name)
    }
    fn clauses(
        &mut self,
        target: SymbolId,
        requires: bool,
        env: &BTreeMap<SymbolId, Sym>,
    ) -> Result<(String, String), String> {
        let mut truth = "true".to_string();
        let mut safe = "true".to_string();
        for declaration in self.program.declarations() {
            if let DeclarationKind::Pact {
                target: id,
                clauses,
            } = &declaration.kind
            {
                if *id != target {
                    continue;
                }
                for clause in clauses.iter().filter(|c| c.requires == requires) {
                    let value = self.expr(&clause.condition, env)?;
                    safe = and([safe, ite(&truth, &value.safe, "true")]);
                    truth = and([truth, value.term]);
                }
            }
        }
        Ok((truth, safe))
    }
    fn invoke(&mut self, id: SymbolId, args: Vec<Sym>, guarded: bool) -> Result<Sym, String> {
        self.budget()?;
        if self.active.contains(&id) || self.active.len() > 32 {
            return Err("recursive calls are unsupported".into());
        }
        let declaration = self.function(id)?;
        let DeclarationKind::Function {
            type_parameters,
            parameters,
            result,
            effects,
            body,
        } = &declaration.kind
        else {
            unreachable!()
        };
        if !type_parameters.is_empty() || effects.as_ref().is_some_and(|e| !e.is_empty()) {
            return Err("generic/effectful callees are unsupported".into());
        }
        self.sort(result)?;
        let mut env: BTreeMap<_, _> = parameters.iter().copied().zip(args).collect();
        self.active.push(id);
        let (pre, pre_safe) = if guarded {
            self.clauses(id, true, &env)?
        } else {
            ("true".into(), "true".into())
        };
        let states = self.block(
            body,
            vec![State {
                path: "true".into(),
                env: env.clone(),
                value: None,
            }],
        )?;
        let mut returned = Vec::new();
        for state in states {
            let prior_safe = state
                .env
                .get(&SymbolId(usize::MAX))
                .map(|s| s.safe.clone())
                .unwrap_or_else(|| "true".into());
            let value = state.value.unwrap_or(Sym {
                term: format!("{}_unit", self.sort(&ValueType::Unit)?),
                ty: ValueType::Unit,
                safe: prior_safe,
            });
            if &value.ty != result {
                return Err("unhandled fallthrough".into());
            }
            returned.push((state.path, value));
        }
        let Some((_, last)) = returned.last() else {
            return Err("no return paths".into());
        };
        let mut term = last.term.clone();
        let mut safe = Vec::new();
        for (path, value) in returned.iter().rev() {
            term = ite(path, &value.term, &term);
            if term.len() > 262_144 {
                return Err("symbolic term size limit exceeded".into());
            }
            safe.push(ite(path, &value.safe, "true"));
        }
        let mut value = Sym {
            term,
            ty: result.clone(),
            safe: and([pre_safe, pre, conjunction(safe)]),
        };
        if guarded {
            for symbol in &self.program.resolution().symbols {
                if symbol.kind == SymbolKind::Result && symbol.parent == Some(id) {
                    env.insert(symbol.id, value.clone());
                }
            }
            let (post, post_safe) = self.clauses(id, false, &env)?;
            value.safe = and([value.safe, post_safe, post]);
        }
        self.active.pop();
        if value.term.len() + value.safe.len() > 262_144 {
            return Err("symbolic term size limit exceeded".into());
        }
        Ok(value)
    }
    fn block(&mut self, block: &Block, mut states: Vec<State>) -> Result<Vec<State>, String> {
        for statement in &block.statements {
            let mut next = Vec::new();
            for state in states {
                if state.value.is_some() {
                    next.push(state);
                } else {
                    next.extend(self.statement(statement, state)?);
                }
            }
            states = next;
            if states.len() > 256 {
                return Err("symbolic path limit exceeded".into());
            }
        }
        Ok(states)
    }
    fn statement(&mut self, statement: &Statement, mut state: State) -> Result<Vec<State>, String> {
        self.budget()?;
        match &statement.kind {
            StatementKind::Let { symbol, value, .. } | StatementKind::Assign { symbol, value } => {
                let value = self.expr(value, &state.env)?;
                // Preserve eager evaluation failures even when this binding is unused.
                let prior = state
                    .env
                    .get(&SymbolId(usize::MAX))
                    .map(|s| s.safe.clone())
                    .unwrap_or_else(|| "true".into());
                state.env.insert(
                    SymbolId(usize::MAX),
                    Sym {
                        term: "true".into(),
                        ty: ValueType::Bool,
                        safe: and([prior, value.safe.clone()]),
                    },
                );
                state.env.insert(*symbol, value);
            }
            StatementKind::Expression(expr) => {
                let value = self.expr(expr, &state.env)?;
                let prior = state
                    .env
                    .get(&SymbolId(usize::MAX))
                    .map(|s| s.safe.clone())
                    .unwrap_or_else(|| "true".into());
                state.env.insert(
                    SymbolId(usize::MAX),
                    Sym {
                        term: "true".into(),
                        ty: ValueType::Bool,
                        safe: and([prior, value.safe]),
                    },
                );
            }
            StatementKind::Return(expr) => {
                let mut value = if let Some(expr) = expr {
                    self.expr(expr, &state.env)?
                } else {
                    Sym {
                        term: format!("{}_unit", self.sort(&ValueType::Unit)?),
                        ty: ValueType::Unit,
                        safe: "true".into(),
                    }
                };
                if let Some(prior) = state.env.get(&SymbolId(usize::MAX)) {
                    value.safe = and([prior.safe.clone(), value.safe]);
                }
                state.value = Some(value);
            }
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition = self.expr(condition, &state.env)?;
                let prior = state
                    .env
                    .get(&SymbolId(usize::MAX))
                    .map(|s| s.safe.clone())
                    .unwrap_or_else(|| "true".into());
                state.env.insert(
                    SymbolId(usize::MAX),
                    Sym {
                        term: "true".into(),
                        ty: ValueType::Bool,
                        safe: and([prior, condition.safe]),
                    },
                );
                let mut yes = state.clone();
                yes.path = and([yes.path, condition.term.clone()]);
                state.path = and([state.path, not(&condition.term)]);
                let mut output = self.block(then_branch, vec![yes])?;
                output.extend(if let Some(other) = else_branch {
                    self.statement(other, state)?
                } else {
                    vec![state]
                });
                return Ok(output);
            }
            StatementKind::Block(block) => return self.block(block, vec![state]),
            StatementKind::While { .. } | StatementKind::Break | StatementKind::Continue => {
                return Err("loops and loop control are unsupported".into());
            }
        }
        Ok(vec![state])
    }
    fn expr(&mut self, expr: &Expression, env: &BTreeMap<SymbolId, Sym>) -> Result<Sym, String> {
        self.budget()?;
        let sort = self.sort(&expr.ty)?;
        let mut safe = "true".to_string();
        let term = match &expr.kind {
            ExpressionKind::Bool(value) => value.to_string(),
            ExpressionKind::I64(value) => bv(*value),
            ExpressionKind::Unit => format!("{sort}_unit"),
            ExpressionKind::Read(id) => {
                return env
                    .get(id)
                    .cloned()
                    .ok_or_else(|| "unbound symbolic value".into());
            }
            ExpressionKind::Construct { variant, arguments } => {
                let values = arguments
                    .iter()
                    .map(|e| self.expr(e, env))
                    .collect::<Result<Vec<_>, _>>()?;
                safe = conjunction(values.iter().map(|v| v.safe.clone()).collect());
                if values.is_empty() {
                    format!("{sort}_v{}", variant.0)
                } else {
                    format!(
                        "({sort}_v{} {})",
                        variant.0,
                        values
                            .iter()
                            .map(|v| v.term.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                }
            }
            ExpressionKind::Record { fields } => {
                let mut values = BTreeMap::new();
                let mut checks = Vec::new();
                for (id, expr) in fields {
                    let value = self.expr(expr, env)?;
                    checks.push(value.safe.clone());
                    values.insert(*id, value);
                }
                safe = conjunction(checks);
                let ordered = self.fields(&expr.ty).ok_or("not a record")?;
                format!(
                    "({sort}_record {})",
                    ordered
                        .iter()
                        .map(|(id, _)| values[id].term.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            }
            ExpressionKind::Field { base, field } => {
                let base = self.expr(base, env)?;
                safe = base.safe;
                format!("({}_f{} {})", self.sort(&base.ty)?, field.0, base.term)
            }
            ExpressionKind::Unary { operator, operand } => {
                let value = self.expr(operand, env)?;
                safe = value.safe;
                match operator {
                    UnaryOp::Not => not(&value.term),
                    UnaryOp::Negate => {
                        safe = and([safe, not(&format!("(= {} {})", value.term, bv(i64::MIN)))]);
                        format!("(bvneg {})", value.term)
                    }
                }
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => {
                let l = self.expr(left, env)?;
                let r = self.expr(right, env)?;
                let (a, b) = (&l.term, &r.term);
                use BinaryOp::*;
                safe = and([l.safe.clone(), r.safe.clone()]);
                match operator {
                    And => {
                        safe = and([l.safe, ite(a, &r.safe, "true")]);
                        format!("(and {a} {b})")
                    }
                    Or => {
                        safe = and([l.safe, ite(a, "true", &r.safe)]);
                        format!("(or {a} {b})")
                    }
                    Equal => format!("(= {a} {b})"),
                    NotEqual => not(&format!("(= {a} {b})")),
                    Less => format!("(bvslt {a} {b})"),
                    LessEqual => format!("(bvsle {a} {b})"),
                    Greater => format!("(bvsgt {a} {b})"),
                    GreaterEqual => format!("(bvsge {a} {b})"),
                    Add | Subtract | Multiply => {
                        let op = match operator {
                            Add => "bvadd",
                            Subtract => "bvsub",
                            _ => "bvmul",
                        };
                        let result = format!("({op} {a} {b})");
                        let fits = format!(
                            "(= ((_ sign_extend 64) {result}) ({op} ((_ sign_extend 64) {a}) ((_ sign_extend 64) {b})))"
                        );
                        safe = and([safe, fits]);
                        result
                    }
                    Divide | Remainder => {
                        safe = and([
                            safe,
                            not(&format!("(= {b} {})", bv(0))),
                            not(&and([
                                format!("(= {a} {})", bv(i64::MIN)),
                                format!("(= {b} {})", bv(-1)),
                            ])),
                        ]);
                        format!(
                            "({} {a} {b})",
                            if *operator == Divide {
                                "bvsdiv"
                            } else {
                                "bvsrem"
                            }
                        )
                    }
                }
            }
            ExpressionKind::Call {
                function,
                arguments,
                type_arguments,
            } => {
                if !type_arguments.is_empty() {
                    return Err("generic calls are unsupported".into());
                }
                let args = arguments
                    .iter()
                    .map(|e| self.expr(e, env))
                    .collect::<Result<Vec<_>, _>>()?;
                let arg_safe = conjunction(args.iter().map(|a| a.safe.clone()).collect());
                let mut result = self.invoke(*function, args, true)?;
                result.safe = and([arg_safe, result.safe]);
                return Ok(result);
            }
            ExpressionKind::Match { scrutinee, arms } => {
                let value = self.expr(scrutinee, env)?;
                let mut remaining = "true".to_string();
                let mut alternatives = Vec::new();
                let mut checks = vec![value.safe.clone()];
                for arm in arms {
                    let mut inner = env.clone();
                    let matches = self.pattern(&arm.pattern, &value, &mut inner)?;
                    let selected = and([remaining.clone(), matches.clone()]);
                    let outcome = self.expr(&arm.value, &inner)?;
                    checks.push(ite(&selected, &outcome.safe, "true"));
                    alternatives.push((selected, outcome.term));
                    remaining = and([remaining, not(&matches)]);
                }
                checks.push(not(&remaining));
                safe = conjunction(checks);
                let mut result = alternatives.last().ok_or("empty match")?.1.clone();
                for (condition, value) in alternatives.into_iter().rev() {
                    result = ite(&condition, &value, &result);
                }
                result
            }
            ExpressionKind::String(_) | ExpressionKind::List(_) | ExpressionKind::Index { .. } => {
                return Err("strings, lists and indexing are unsupported".into());
            }
        };
        if term.len() + safe.len() > 262_144 {
            return Err("symbolic term size limit exceeded".into());
        }
        Ok(Sym {
            term,
            ty: expr.ty.clone(),
            safe,
        })
    }
    fn pattern(
        &mut self,
        pattern: &Pattern,
        value: &Sym,
        env: &mut BTreeMap<SymbolId, Sym>,
    ) -> Result<String, String> {
        Ok(match &pattern.kind {
            PatternKind::Wildcard => "true".into(),
            PatternKind::Binding(id) => {
                let mut bound = value.clone();
                bound.safe = "true".into();
                env.insert(*id, bound);
                "true".into()
            }
            PatternKind::Bool(b) => format!("(= {} {b})", value.term),
            PatternKind::I64(i) => format!("(= {} {})", value.term, bv(*i)),
            PatternKind::Variant { variant, payload } => {
                let sort = self.sort(&value.ty)?;
                let mut parts = vec![format!("((_ is {sort}_v{}) {})", variant.0, value.term)];
                let types = self
                    .variants(&value.ty)?
                    .into_iter()
                    .find(|(id, _)| id == variant)
                    .ok_or("unknown constructor")?
                    .1;
                for (i, (pattern, ty)) in payload.iter().zip(types).enumerate() {
                    parts.push(self.pattern(
                        pattern,
                        &Sym {
                            term: format!("({sort}_v{}_p{i} {})", variant.0, value.term),
                            ty,
                            safe: "true".into(),
                        },
                        env,
                    )?);
                }
                conjunction(parts)
            }
            PatternKind::String(_) => return Err("string patterns are unsupported".into()),
        })
    }
}
