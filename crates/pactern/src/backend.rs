//! Freestanding C emission and a deny-import, fuel/memory-bounded Wasm host.
//! ABI v1 is intentionally an invocation artifact: arguments and entry point are
//! bound into the module. The opt-in quota ABI grants only host-issued handles.
use crate::{
    core::*,
    interpreter::{Limits, TrapKind, Value},
    resolution::{SymbolId, SymbolKind},
    syntax::{BinaryOp, UnaryOp},
};
#[path = "backend_quota.rs"]
mod quota;
#[path = "backend_terminal.rs"]
mod terminal;
pub use quota::{QuotaArgument, QuotaExecution, emit_c_quota, run_wasm_quota};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
pub use terminal::{TerminalIo, emit_c_terminal, run_wasm_terminal};

#[derive(Debug)]
pub struct BackendError(pub String);
impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for BackendError {}
type Result<T> = std::result::Result<T, BackendError>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Execution {
    Value(Value),
    Trap {
        kind: TrapKind,
        cause: Option<TrapKind>,
    },
}
#[derive(Clone, Copy, Debug)]
pub struct HostLimits {
    pub fuel: u64,
    pub memory_bytes: usize,
    pub module_bytes: usize,
    pub output_bytes: usize,
}
impl Default for HostLimits {
    fn default() -> Self {
        Self {
            fuel: 100_000_000,
            memory_bytes: 16 * 1024 * 1024,
            module_bytes: 8 * 1024 * 1024,
            output_bytes: 1024 * 1024,
        }
    }
}

pub fn emit_c(
    program: &Program,
    entry: SymbolId,
    args: &[Value],
    limits: Limits,
) -> Result<String> {
    let args: Vec<_> = args.iter().map(Input::Value).collect();
    emit(program, entry, &args, limits, false, Bridge::Pure)
}
pub fn emit_c_guarded(
    program: &Program,
    entry: SymbolId,
    args: &[Value],
    limits: Limits,
) -> Result<String> {
    let args: Vec<_> = args.iter().map(Input::Value).collect();
    emit(program, entry, &args, limits, true, Bridge::Pure)
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bridge {
    Pure,
    Quota,
    Terminal,
}
impl Bridge {
    fn handle(self) -> &'static str {
        match self {
            Self::Pure => "",
            Self::Quota => "host::quota::Txn",
            Self::Terminal => "host::terminal::Terminal",
        }
    }
    fn prefix(self) -> &'static str {
        match self {
            Self::Quota => "pt_quota",
            Self::Terminal => "pt_terminal",
            Self::Pure => "",
        }
    }
    fn permits_effect(self, effect: &str) -> bool {
        match self {
            Self::Pure => false,
            Self::Quota => ["db.read", "db.write"].contains(&effect),
            Self::Terminal => ["terminal.read", "terminal.write", "env.read"].contains(&effect),
        }
    }
}
enum Input<'a> {
    Value(&'a Value),
    HandleSlot(u32),
}
fn emit(
    program: &Program,
    entry: SymbolId,
    args: &[Input<'_>],
    limits: Limits,
    guarded: bool,
    bridge: Bridge,
) -> Result<String> {
    let Some(d) = program.declarations().iter().find(|d| d.symbol == entry) else {
        return Err(BackendError("unknown entry point".into()));
    };
    let DeclarationKind::Function {
        parameters,
        type_parameters,
        ..
    } = &d.kind
    else {
        return Err(BackendError("entry is not a function".into()));
    };
    if !type_parameters.is_empty() || parameters.len() != args.len() {
        return Err(BackendError(
            "invalid entry arguments or generic entry".into(),
        ));
    }
    if !parameters.iter().zip(args).all(|(id, v)| {
        program
            .symbol_types()
            .get(id)
            .is_some_and(|t| match v {
                Input::Value(v)=>crate::interpreter::accepts(program,t,v,0),
                Input::HandleSlot(_)=>bridge != Bridge::Pure && matches!(t,crate::typing::ValueType::Named(id) if program.resolution().symbols[id.0].kind==SymbolKind::BuiltinType && program.resolution().symbols[id.0].name==bridge.handle()),
            })
    }) {
        return Err(BackendError("entry argument type mismatch".into()));
    }
    let mut out = include_str!("../../../runtime/pactern.c").to_string();
    match bridge {
        Bridge::Quota => out.push_str(include_str!("../../../runtime/quota.c")),
        Bridge::Terminal => out.push_str(include_str!("../../../runtime/terminal.c")),
        Bridge::Pure => {}
    }
    for d in program.declarations() {
        if let DeclarationKind::Function {
            parameters,
            effects,
            ..
        } = &d.kind
        {
            if effects.as_ref().is_some_and(|e| {
                e.iter()
                    .any(|e| !bridge.permits_effect(&e.replace("::", ".")))
            }) {
                return Err(BackendError(
                    "effectful functions require a capability host bridge".into(),
                ));
            }
            writeln!(
                out,
                "static V f{}({});",
                d.symbol.0,
                parameters
                    .iter()
                    .map(|p| format!("V a{}", p.0))
                    .collect::<Vec<_>>()
                    .join(",")
            )
            .unwrap();
        }
    }
    for d in program.declarations() {
        if let DeclarationKind::Function {
            parameters, body, ..
        } = &d.kind
        {
            let mut e = Emitter::new(program, bridge);
            for p in parameters {
                e.line(format!("r[{}]=a{};", p.0, p.0));
                if e.is_handle(*p) && !program.borrowed_parameters().contains(p) {
                    e.owned.insert(*p);
                }
            }
            e.line("tick(); if(trap_code)goto cleanup;");
            if guarded {
                e.guards(d.symbol, true)?;
            }
            e.block(body)?;
            e.line("goto returned; returned:;");
            if guarded {
                for s in &program.resolution().symbols {
                    if s.kind == SymbolKind::Result && s.parent == Some(d.symbol) {
                        e.line(format!("charge(r[{}],0);", e.result));
                        e.check();
                        e.line(format!("r[{}]=r[{}];", s.id.0, e.result));
                    }
                }
                e.guards(d.symbol, false)?;
            }
            if e.next > 4096 {
                return Err(BackendError("C root frame exceeds 4096 slots".into()));
            }
            e.line("cleanup:;");
            e.release(e.owned.iter().copied().collect());
            e.line(format!(
                "roots=frame.previous; --depth; active_slots-={};",
                e.next
            ));
            e.line(format!("return r[{}];", e.result));
            writeln!(out,"static V f{}({}) {{\n if(depth>=max_depth || active_slots>32768-{}){{fail(4);return unit();}} ++depth; active_slots+={};\n V r[{}]={{0}}; Roots frame={{roots,r,{}}}; roots=&frame;\n{}\n}}",d.symbol.0,parameters.iter().map(|p|format!("V a{}",p.0)).collect::<Vec<_>>().join(","),e.next,e.next,e.next,e.next,e.code).unwrap();
        }
    }
    let mut e = Emitter::new(program, bridge);
    let mut input = vec![];
    for v in args {
        let slot = match v {
            Input::Value(v) => e.value(v)?,
            Input::HandleSlot(slot) => {
                let n = e.slot();
                e.line(format!(
                    "r[{n}]=scalar(7,{}_root({slot}));",
                    bridge.prefix()
                ));
                n
            }
        };
        e.line(format!("charge(r[{slot}],0);"));
        e.check();
        input.push(slot);
    }
    e.line(format!(
        "r[{}]=f{}({});",
        e.result,
        entry.0,
        input
            .iter()
            .map(|i| format!("r[{i}]"))
            .collect::<Vec<_>>()
            .join(",")
    ));
    e.line(format!("if(!trap_code)encode(r[{}],0);", e.result));
    e.line("cleanup: roots=0; active_slots=0; collect(); return trap_code;");
    if e.next > 4096 {
        return Err(BackendError(
            "C invocation root frame exceeds 4096 slots".into(),
        ));
    }
    writeln!(out,"unsigned int pt_run(void) {{ reset({}ULL,{}); V r[{}]={{0}};Roots frame={{0,r,{}}};roots=&frame; active_slots={}; {} }}",limits.fuel,limits.call_depth.min(64),e.next,e.next,e.next,e.code).unwrap();
    Ok(out)
}
struct Emitter<'a> {
    program: &'a Program,
    code: String,
    next: usize,
    result: usize,
    labels: usize,
    bridge: Bridge,
    owned: BTreeSet<SymbolId>,
    scopes: Vec<Vec<SymbolId>>,
    loop_scopes: Vec<usize>,
}
impl<'a> Emitter<'a> {
    fn new(program: &'a Program, bridge: Bridge) -> Self {
        let result = program.resolution().symbols.len();
        Self {
            program,
            code: String::new(),
            next: result + 1,
            result,
            labels: 0,
            bridge,
            owned: BTreeSet::new(),
            scopes: vec![],
            loop_scopes: vec![],
        }
    }
    fn line(&mut self, s: impl AsRef<str>) {
        self.code.push_str(s.as_ref());
        self.code.push('\n');
    }
    fn slot(&mut self) -> usize {
        let n = self.next;
        self.next += 1;
        n
    }
    fn check(&mut self) {
        self.line("if(trap_code)goto cleanup;");
    }
    fn is_handle(&self, id: SymbolId) -> bool {
        self.bridge != Bridge::Pure
            && matches!(self.program.symbol_types().get(&id),Some(crate::typing::ValueType::Named(t)) if self.program.resolution().symbols[t.0].name==self.bridge.handle())
    }
    fn release(&mut self, ids: Vec<SymbolId>) {
        for id in ids.into_iter().rev() {
            self.line(format!(
                "if(r[{}].tag==7){{{}_release((U32)r[{}].n);r[{}]=unit();}}",
                id.0,
                self.bridge.prefix(),
                id.0,
                id.0
            ));
        }
    }
    fn release_scopes(&mut self, start: usize) {
        let ids = self.scopes[start..].iter().flatten().copied().collect();
        self.release(ids);
    }
    fn guards(&mut self, target: SymbolId, requires: bool) -> Result<()> {
        let clauses: Vec<_> = self
            .program
            .declarations()
            .iter()
            .filter_map(|d| match &d.kind {
                DeclarationKind::Pact { target: t, clauses } if *t == target => Some(clauses),
                _ => None,
            })
            .flatten()
            .filter(|c| c.requires == requires)
            .cloned()
            .collect();
        for c in clauses {
            self.line("++guard_depth;");
            let v = self.expr(&c.condition)?;
            self.line("--guard_depth;");
            self.line(format!(
                "if(!r[{v}].n){{fail({});goto cleanup;}}",
                if requires { 7 } else { 8 }
            ));
        }
        Ok(())
    }
    fn block(&mut self, b: &Block) -> Result<()> {
        self.scopes.push(vec![]);
        for s in &b.statements {
            self.statement(s)?;
        }
        let ids = self.scopes.pop().unwrap();
        self.release(ids);
        Ok(())
    }
    fn statement(&mut self, s: &Statement) -> Result<()> {
        self.line("tick();");
        self.check();
        match &s.kind {
            StatementKind::Let { symbol, value, .. } | StatementKind::Assign { symbol, value } => {
                let i = self.expr(value)?;
                self.line(format!("r[{}]=r[{i}];", symbol.0));
                if self.is_handle(*symbol) {
                    self.owned.insert(*symbol);
                    if matches!(s.kind, StatementKind::Let { .. }) {
                        self.scopes.last_mut().unwrap().push(*symbol);
                    }
                }
            }
            StatementKind::Expression(e) => {
                self.expr(e)?;
            }
            StatementKind::Return(value) => {
                if let Some(v) = value {
                    let i = self.expr(v)?;
                    self.line(format!("r[{}]=r[{i}];", self.result));
                }
                self.release_scopes(0);
                self.line("goto returned;");
            }
            StatementKind::Break => {
                self.release_scopes(*self.loop_scopes.last().unwrap());
                self.line("break;");
            }
            StatementKind::Continue => {
                self.release_scopes(*self.loop_scopes.last().unwrap());
                self.line("continue;");
            }
            StatementKind::Block(b) => self.block(b)?,
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let i = self.expr(condition)?;
                self.line(format!("if(r[{i}].n){{"));
                self.block(then_branch)?;
                self.line("}else{");
                if let Some(s) = else_branch {
                    self.statement(s)?;
                }
                self.line("}");
            }
            StatementKind::While { condition, body } => {
                self.line("while(1){");
                let i = self.expr(condition)?;
                self.line(format!("if(!r[{i}].n)break;"));
                self.loop_scopes.push(self.scopes.len());
                self.block(body)?;
                self.loop_scopes.pop();
                self.line("}");
            }
        }
        Ok(())
    }
    fn value(&mut self, v: &Value) -> Result<usize> {
        let n = self.slot();
        match v {
            Value::Bool(b) => self.line(format!("r[{n}]=scalar(1,{});", u8::from(*b))),
            Value::I64(i) => self.line(format!("r[{n}]=scalar(2,{});", integer(*i))),
            Value::Unit => self.line(format!("r[{n}]=unit();")),
            Value::String(s) => self.string(n, s),
            Value::List(vs) => {
                let mut inputs = vec![];
                for v in vs {
                    inputs.push(self.value(v)?);
                }
                self.container(n, 4, 0, &inputs);
            }
            Value::Record(fs) => {
                let mut inputs = vec![];
                for (id, v) in fs {
                    inputs.push((*id, self.value(v)?));
                }
                self.record(n, &inputs);
            }
            Value::Variant { variant, payload } => {
                let mut inputs = vec![];
                for v in payload {
                    inputs.push(self.value(v)?);
                }
                self.container(n, 6, variant.0, &inputs);
            }
        }
        self.check();
        Ok(n)
    }
    fn string(&mut self, n: usize, s: &str) {
        let escaped = s
            .as_bytes()
            .iter()
            .map(|b| format!("\\{b:03o}"))
            .collect::<String>();
        self.line(format!(
            "r[{n}]=string((const unsigned char*)\"{escaped}\",{});",
            s.len()
        ));
    }
    fn container(&mut self, n: usize, tag: u8, aux: usize, inputs: &[usize]) {
        self.line(format!("r[{n}]=allocate({tag},{},{aux});", inputs.len()));
        self.check();
        for (j, i) in inputs.iter().enumerate() {
            self.line(format!("items(r[{n}])[{j}]=r[{i}];"));
        }
    }
    fn record(&mut self, n: usize, inputs: &[(SymbolId, usize)]) {
        self.line(format!("r[{n}]=allocate(5,{},0);", inputs.len()));
        self.check();
        for (j, (id, i)) in inputs.iter().enumerate() {
            self.line(format!("fields(r[{n}])[{j}]=(Field){{{},0,r[{i}]}};", id.0));
        }
    }
    fn expr(&mut self, e: &Expression) -> Result<usize> {
        self.expr_mode(e, false)
    }
    fn expr_mode(&mut self, e: &Expression, borrowed: bool) -> Result<usize> {
        let n = self.slot();
        self.line("tick();");
        self.check();
        match &e.kind {
            ExpressionKind::Bool(b) => self.line(format!("r[{n}]=scalar(1,{});", u8::from(*b))),
            ExpressionKind::I64(i) => self.line(format!("r[{n}]=scalar(2,{});", integer(*i))),
            ExpressionKind::String(s) => self.string(n, s),
            ExpressionKind::Unit => self.line(format!("r[{n}]=unit();")),
            ExpressionKind::Read(id) => {
                self.line(format!("r[{n}]=r[{}];", id.0));
                if self.is_handle(*id) && !borrowed {
                    self.line(format!("r[{}]=unit();", id.0));
                }
            }
            ExpressionKind::List(vs) => {
                let mut inputs = vec![];
                for e in vs {
                    inputs.push(self.expr(e)?);
                }
                self.container(n, 4, 0, &inputs);
            }
            ExpressionKind::Record { fields } => {
                let mut inputs = vec![];
                for (id, e) in fields {
                    inputs.push((*id, self.expr(e)?));
                }
                self.record(n, &inputs);
            }
            ExpressionKind::Construct { variant, arguments } => {
                let mut inputs = vec![];
                for e in arguments {
                    inputs.push(self.expr(e)?);
                }
                self.container(n, 6, variant.0, &inputs);
            }
            ExpressionKind::Call {
                function,
                arguments,
                ..
            } => {
                let external = !self
                    .program
                    .declarations()
                    .iter()
                    .any(|d| d.symbol == *function);
                if external && self.bridge == Bridge::Pure {
                    return Err(BackendError(
                        "external host calls require a capability host bridge".into(),
                    ));
                }
                let mut inputs = vec![];
                for (i, e) in arguments.iter().enumerate() {
                    let borrowed = if external {
                        i == 0
                            && [
                                "host::quota::read",
                                "host::quota::reserve",
                                "host::terminal::args",
                                "host::terminal::arg_count",
                                "host::terminal::read_line",
                                "host::terminal::write",
                                "host::terminal::write_error",
                            ]
                            .contains(&self.program.resolution().symbols[function.0].name.as_str())
                    } else {
                        self.program.declarations().iter().find(|d|d.symbol==*function).is_some_and(|d|matches!(&d.kind,DeclarationKind::Function{parameters,..} if parameters.get(i).is_some_and(|p|self.program.borrowed_parameters().contains(p))))
                    };
                    inputs.push(self.expr_mode(e, borrowed)?);
                }
                if external {
                    let name = &self.program.resolution().symbols[function.0].name;
                    let token = inputs
                        .first()
                        .ok_or_else(|| BackendError("host call is missing a handle".into()))?;
                    if !name.starts_with(match self.bridge {
                        Bridge::Quota => "host::quota::",
                        Bridge::Terminal => "host::terminal::",
                        Bridge::Pure => "",
                    }) {
                        return Err(BackendError(
                            "host call requires a different capability bridge".into(),
                        ));
                    }
                    match name.as_str() {
                        "host::terminal::args" => {
                            self.line(format!("r[{n}]=terminal_args((U32)r[{token}].n);"))
                        }
                        "host::terminal::arg_count" => self.line(format!(
                            "r[{n}]=scalar(2,pt_terminal_argc((U32)r[{token}].n));"
                        )),
                        "host::terminal::read_line" => {
                            let variant = |name: &str| {
                                self.program
                                    .resolution()
                                    .symbols
                                    .iter()
                                    .find(|s| {
                                        s.kind == SymbolKind::BuiltinVariant && s.name == name
                                    })
                                    .unwrap()
                                    .id
                                    .0
                            };
                            self.line(format!(
                                "r[{n}]=terminal_read_line((U32)r[{token}].n,{},{});",
                                variant("Some"),
                                variant("None")
                            ));
                        }
                        "host::terminal::write" | "host::terminal::write_error" => {
                            let text = inputs[1];
                            self.line(format!("pt_terminal_write((U32)r[{token}].n,{},(U32)(unsigned long)(object(r[{text}])+1),object(r[{text}])->len);r[{n}]=unit();", u8::from(name.ends_with("write_error"))));
                        }
                        "host::quota::read" => self.line(format!(
                            "r[{n}]=scalar(2,pt_quota_read((U32)r[{token}].n));"
                        )),
                        "host::quota::reserve" => {
                            let amount = inputs.get(1).ok_or_else(|| {
                                BackendError("reserve is missing an amount".into())
                            })?;
                            self.line(format!("r[{n}]=scalar(1,pt_quota_reserve((U32)r[{token}].n,r[{amount}].n));"));
                        }
                        "host::quota::commit" => {
                            self.line(format!("pt_quota_commit((U32)r[{token}].n);r[{n}]=unit();"))
                        }
                        "host::quota::rollback" => self.line(format!(
                            "pt_quota_rollback((U32)r[{token}].n);r[{n}]=unit();"
                        )),
                        _ => return Err(BackendError("unregistered host call".into())),
                    }
                } else {
                    self.line(format!(
                        "r[{n}]=f{}({});",
                        function.0,
                        inputs
                            .iter()
                            .map(|i| format!("r[{i}]"))
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                }
            }
            ExpressionKind::Field { base, field } => {
                let a = self.expr(base)?;
                self.line(format!("r[{n}]=field(r[{a}],{});", field.0));
            }
            ExpressionKind::Index { base, index } => {
                let a = self.expr(base)?;
                let b = self.expr(index)?;
                self.line(format!("r[{n}]=index_value(r[{a}],r[{b}]);"));
            }
            ExpressionKind::Unary { operator, operand } => {
                let a = self.expr(operand)?;
                self.line(format!(
                    "r[{n}]=unary({},r[{a}]);",
                    if *operator == UnaryOp::Not { 0 } else { 1 }
                ));
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => {
                let a = self.expr(left)?;
                if matches!(operator, BinaryOp::And | BinaryOp::Or) {
                    self.line(format!(
                        "r[{n}]=r[{a}];if({}r[{a}].n){{",
                        if *operator == BinaryOp::Or { "!" } else { "" }
                    ));
                    let b = self.expr(right)?;
                    self.line(format!("r[{n}]=r[{b}];}}"));
                } else {
                    let b = self.expr(right)?;
                    self.line(format!(
                        "r[{n}]=binary({},r[{a}],r[{b}]);",
                        binary_code(*operator)
                    ));
                }
            }
            ExpressionKind::Match { scrutinee, arms } => {
                let a = self.expr(scrutinee)?;
                let label = self.labels;
                self.labels += 1;
                for arm in arms {
                    let mut bindings = vec![];
                    let condition =
                        self.pattern(&arm.pattern, &format!("r[{a}]"), &mut bindings)?;
                    self.line(format!("if({condition}){{"));
                    for (id, v) in bindings {
                        self.line(format!("charge({v},0);"));
                        self.check();
                        self.line(format!("r[{}]={v};", id.0));
                    }
                    let b = self.expr(&arm.value)?;
                    self.line(format!("r[{n}]=r[{b}];goto match_{label};}}"));
                }
                self.line(format!("fail(6);goto cleanup;match_{label}:;"));
            }
        }
        self.line(format!("charge(r[{n}],0);"));
        self.check();
        Ok(n)
    }
    fn pattern(
        &mut self,
        p: &Pattern,
        value: &str,
        bindings: &mut Vec<(SymbolId, String)>,
    ) -> Result<String> {
        Ok(match &p.kind {
            PatternKind::Wildcard => "1".into(),
            PatternKind::Binding(id) => {
                bindings.push((*id, value.into()));
                "1".into()
            }
            PatternKind::Bool(b) => format!("({value}).n=={}", u8::from(*b)),
            PatternKind::I64(i) => format!("({value}).n=={}", integer(*i)),
            PatternKind::String(s) => {
                let n = self.slot();
                self.string(n, s);
                self.check();
                format!("equal({value},r[{n}],0)")
            }
            PatternKind::Variant { variant, payload } => {
                let mut tests = vec![
                    format!("({value}).tag==6"),
                    format!("object({value})->aux=={}", variant.0),
                    format!("object({value})->len=={}", payload.len()),
                ];
                for (i, p) in payload.iter().enumerate() {
                    tests.push(self.pattern(p, &format!("items({value})[{i}]"), bindings)?);
                }
                tests
                    .iter()
                    .map(|s| format!("({s})"))
                    .collect::<Vec<_>>()
                    .join("&&")
            }
        })
    }
}
fn integer(i: i64) -> String {
    if i == i64::MIN {
        "(-9223372036854775807LL-1)".into()
    } else {
        format!("{i}LL")
    }
}
fn binary_code(op: BinaryOp) -> u8 {
    use BinaryOp::*;
    match op {
        Add => 0,
        Subtract => 1,
        Multiply => 2,
        Divide => 3,
        Remainder => 4,
        Equal => 5,
        NotEqual => 6,
        Less => 7,
        LessEqual => 8,
        Greater => 9,
        GreaterEqual => 10,
        And => 11,
        Or => 12,
    }
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn temporary() -> Result<Temporary> {
    for _ in 0..100 {
        let path = std::env::temp_dir().join(format!(
            "pactern-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(Temporary(path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(BackendError(e.to_string())),
        }
    }
    Err(BackendError("cannot allocate build directory".into()))
}
/// Resolve explicit toolchain overrides first; then packaged LLVM and PATH.
/// The compiler is trusted build-time tooling, never an authority granted to Wasm.
pub fn compiler_path() -> PathBuf {
    if let Some(path) = std::env::var_os("PACTERN_CLANG") {
        return path.into();
    }
    for path in [
        "/opt/homebrew/opt/llvm@21/bin/clang",
        "/opt/homebrew/opt/llvm/bin/clang",
        "/usr/bin/clang-21",
        "/usr/bin/clang-20",
    ] {
        if std::path::Path::new(path).exists() {
            return path.into();
        }
    }
    "clang".into()
}
pub fn compiler_version() -> Result<String> {
    let output = Command::new(compiler_path())
        .arg("--version")
        .output()
        .map_err(|e| BackendError(e.to_string()))?;
    if !output.status.success() {
        return Err(BackendError("clang --version failed".into()));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string())
}
pub fn linker_path() -> PathBuf {
    if let Some(linker) = std::env::var_os("PACTERN_WASM_LD") {
        PathBuf::from(linker)
    } else {
        let mut found = PathBuf::from("wasm-ld");
        for linker in [
            "/opt/homebrew/opt/lld@21/bin/wasm-ld",
            "/opt/homebrew/opt/lld/bin/wasm-ld",
        ] {
            if std::path::Path::new(linker).exists() {
                found = PathBuf::from(linker);
                break;
            }
        }
        found
    }
}
/// Compile a standalone, import-free core Wasm module. No WASI is linked.
pub fn compile_wasm(c: &str) -> Result<Vec<u8>> {
    let temp = temporary()?;
    let input = temp.0.join("guest.c");
    let object = temp.0.join("guest.o");
    let output = temp.0.join("guest.wasm");
    std::fs::write(&input, c).map_err(|e| BackendError(e.to_string()))?;
    let mut command = Command::new(compiler_path());
    command.args([
        "--target=wasm32-unknown-unknown",
        "-std=c11",
        "-O1",
        "-ffreestanding",
        "-fno-builtin",
        "-c",
    ]);
    let result = command
        .arg(&input)
        .arg("-o")
        .arg(&object)
        .output()
        .map_err(|e| BackendError(format!("cannot run clang (set PACTERN_CLANG): {e}")))?;
    if !result.status.success() {
        return Err(BackendError(format!(
            "C/Wasm compilation failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    let result = Command::new(linker_path())
        .args([
            "--no-entry",
            "--export=pt_run",
            "--export=pt_abi_version",
            "--export=pt_output_ptr",
            "--export=pt_output_len",
            "--export=pt_trap_cause",
            "--export=pt_collections",
            "--export-memory",
            "-z",
            "stack-size=1048576",
            "--max-memory=16777216",
        ])
        .arg(&object)
        .arg("-o")
        .arg(&output)
        .output()
        .map_err(|e| BackendError(format!("cannot run wasm-ld (set PACTERN_WASM_LD): {e}")))?;
    if !result.status.success() {
        return Err(BackendError(format!(
            "Wasm linking failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    std::fs::read(output).map_err(|e| BackendError(e.to_string()))
}
fn trap(code: u32) -> Result<TrapKind> {
    Ok(match code {
        1 => TrapKind::Overflow,
        2 => TrapKind::DivisionByZero,
        3 => TrapKind::IndexOutOfBounds,
        4 => TrapKind::ExecutionLimit,
        5 => TrapKind::InvalidInvocation,
        6 => TrapKind::Invariant,
        7 => TrapKind::Precondition,
        8 => TrapKind::Postcondition,
        9 => TrapKind::ContractEvaluation,
        _ => return Err(BackendError(format!("invalid guest trap code {code}"))),
    })
}
/// Every import is rejected, including unused imports, memories and WASI.
/// Fuel includes module initialization. Store destruction owns all cleanup.
pub fn run_wasm(bytes: &[u8], limits: HostLimits) -> Result<Execution> {
    use wasmi::{Config, Engine, Linker, Module, Store, StoreLimitsBuilder};
    if bytes.len() > limits.module_bytes {
        return Err(BackendError("module byte limit exceeded".into()));
    }
    let mut config = Config::default();
    config.consume_fuel(true);
    config.compilation_mode(wasmi::CompilationMode::Eager);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, bytes).map_err(|e| BackendError(e.to_string()))?;
    if let Some(import) = module.imports().next() {
        return Err(BackendError(format!(
            "ambient import denied: {}::{}",
            import.module(),
            import.name()
        )));
    }
    let resource_limits = StoreLimitsBuilder::new()
        .memory_size(limits.memory_bytes)
        .memories(1)
        .tables(1)
        .table_elements(1024)
        .instances(1)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(&engine, resource_limits);
    store.limiter(|state| state);
    store
        .set_fuel(limits.fuel)
        .map_err(|e| BackendError(e.to_string()))?;
    let instance = Linker::new(&engine)
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| BackendError(format!("module initialization rejected: {e}")))?;
    execute_instance(store, instance, limits, 0, |_| Ok(()))
}
fn execute_instance<T>(
    mut store: wasmi::Store<T>,
    instance: wasmi::Instance,
    limits: HostLimits,
    mut reserve_fuel: u64,
    mut checkpoint: impl FnMut(&mut T) -> Result<()>,
) -> Result<Execution> {
    checkpoint(store.data_mut())?;
    let getter = |name: &str| {
        instance
            .get_typed_func::<(), u32>(&store, name)
            .map_err(|e| BackendError(format!("invalid ABI export {name}: {e}")))
    };
    let version = getter("pt_abi_version")?;
    let run = getter("pt_run")?;
    let cause = getter("pt_trap_cause")?;
    let ptr = getter("pt_output_ptr")?;
    let len = getter("pt_output_len")?;
    if version
        .call(&mut store, ())
        .map_err(|e| BackendError(e.to_string()))?
        != 1
    {
        return Err(BackendError("unsupported guest ABI".into()));
    }
    let mut call = run
        .call_resumable(&mut store, ())
        .map_err(|e| BackendError(format!("Wasm trap: {e}")))?;
    let code = loop {
        match call {
            wasmi::TypedResumableCall::Finished(code) => {
                checkpoint(store.data_mut())?;
                break code;
            }
            wasmi::TypedResumableCall::HostTrap(trap) => {
                return Err(BackendError(format!(
                    "Wasm host trap: {}",
                    trap.host_error()
                )));
            }
            wasmi::TypedResumableCall::OutOfFuel(paused) => {
                checkpoint(store.data_mut())?;
                if reserve_fuel == 0 {
                    return Ok(Execution::Trap {
                        kind: TrapKind::ExecutionLimit,
                        cause: None,
                    });
                }
                let grant = reserve_fuel.min(10_000.max(paused.required_fuel()));
                reserve_fuel -= grant;
                let leftover = store.get_fuel().map_err(|e| BackendError(e.to_string()))?;
                store
                    .set_fuel(leftover + grant)
                    .map_err(|e| BackendError(e.to_string()))?;
                call = paused
                    .resume(&mut store)
                    .map_err(|e| BackendError(format!("Wasm trap: {e}")))?;
            }
        }
    };
    if code != 0 {
        let c = cause
            .call(&mut store, ())
            .map_err(|e| BackendError(e.to_string()))?;
        return Ok(Execution::Trap {
            kind: trap(code)?,
            cause: if c == 0 { None } else { Some(trap(c)?) },
        });
    }
    let length = len
        .call(&mut store, ())
        .map_err(|e| BackendError(e.to_string()))? as usize;
    let offset = ptr
        .call(&mut store, ())
        .map_err(|e| BackendError(e.to_string()))? as usize;
    if length > limits.output_bytes {
        return Err(BackendError("guest output limit exceeded".into()));
    }
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or_else(|| BackendError("missing guest memory".into()))?;
    let mut output = vec![0; length];
    memory
        .read(&store, offset, &mut output)
        .map_err(|e| BackendError(format!("invalid guest output: {e}")))?;
    let mut decoder = Decoder {
        bytes: &output,
        position: 0,
    };
    let value = decoder.value(0)?;
    if decoder.position != output.len() {
        return Err(BackendError("trailing guest output bytes".into()));
    }
    Ok(Execution::Value(value))
}
struct Decoder<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl Decoder<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self
            .position
            .checked_add(n)
            .ok_or_else(|| BackendError("guest output overflow".into()))?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| BackendError("truncated guest output".into()))?;
        self.position = end;
        Ok(bytes)
    }
    fn number(&mut self, n: usize) -> Result<u64> {
        let mut out = 0;
        for (i, b) in self.take(n)?.iter().enumerate() {
            out |= u64::from(*b) << (8 * i);
        }
        Ok(out)
    }
    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > 64 {
            return Err(BackendError("guest value nesting limit exceeded".into()));
        }
        let tag = self.number(1)?;
        Ok(match tag {
            0 => Value::Unit,
            1 => {
                let n = self.number(8)?;
                if n > 1 {
                    return Err(BackendError("invalid guest boolean".into()));
                }
                Value::Bool(n == 1)
            }
            2 => Value::I64(self.number(8)? as i64),
            3 => {
                let n = self.number(4)? as usize;
                Value::String(
                    std::str::from_utf8(self.take(n)?)
                        .map_err(|e| BackendError(e.to_string()))?
                        .into(),
                )
            }
            4 | 6 => {
                let n = self.number(4)? as usize;
                let variant = if tag == 6 {
                    self.number(4)? as usize
                } else {
                    0
                };
                if n > self.bytes.len().saturating_sub(self.position) {
                    return Err(BackendError("invalid guest element count".into()));
                }
                let mut values = Vec::with_capacity(n);
                for _ in 0..n {
                    values.push(self.value(depth + 1)?);
                }
                if tag == 4 {
                    Value::List(values)
                } else {
                    Value::Variant {
                        variant: SymbolId(variant),
                        payload: values,
                    }
                }
            }
            5 => {
                let n = self.number(4)?;
                let mut values = BTreeMap::new();
                for _ in 0..n {
                    let id = SymbolId(self.number(4)? as usize);
                    let value = self.value(depth + 1)?;
                    if values.insert(id, value).is_some() {
                        return Err(BackendError("duplicate guest record field".into()));
                    }
                }
                Value::Record(values)
            }
            _ => return Err(BackendError("invalid guest value tag".into())),
        })
    }
}
