//! Revision-bound pact inventories and deterministic execution evidence.
use crate::{
    core::{DeclarationKind, Program},
    interpreter::{self, Limits, Trap, TrapKind, Value},
    resolution::SymbolId,
    verification::{self, Solver, Status},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Supported,
    Unsupported,
    Inconsistent,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Obligation {
    pub id: String,
    pub pact: usize,
    pub target: usize,
    pub clause: usize,
    pub phase: String,
    pub path: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub support: Support,
    pub reason: Option<String>,
    /// This is eligibility, not a claim that a concrete artifact contains a guard.
    pub runtime_guard_eligible: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub revision: String,
    pub obligations: Vec<Obligation>,
}
/// IDs bind the exact supplied source/dependency revision and declaration location.
/// The caller must use a complete input revision, not a timestamp or mutable label.
pub fn inventory(program: &Program, revision: &str) -> Inventory {
    let mut obligations = Vec::new();
    for declaration in program.declarations() {
        let DeclarationKind::Pact { target, clauses } = &declaration.kind else {
            continue;
        };
        let supported = verification::queries(program, *target);
        for (index, clause) in clauses.iter().enumerate() {
            let path = program.resolution().files[clause.origin.file.0].clone();
            let mut digest = Sha256::new();
            for field in [
                "pactern/obligation/1".to_owned(),
                revision.to_owned(),
                path.clone(),
                declaration.symbol.0.to_string(),
                index.to_string(),
                clause.origin.span.start.byte.to_string(),
                clause.origin.span.end.byte.to_string(),
            ] {
                digest.update((field.len() as u64).to_be_bytes());
                digest.update(field.as_bytes());
            }
            obligations.push(Obligation {
                id: format!("obl_{:x}", digest.finalize()),
                pact: declaration.symbol.0,
                target: target.0,
                clause: index,
                phase: if clause.requires {
                    "requires"
                } else {
                    "ensures"
                }
                .into(),
                path,
                start_byte: clause.origin.span.start.byte,
                end_byte: clause.origin.span.end.byte,
                support: if supported.is_ok() {
                    Support::Supported
                } else {
                    Support::Unsupported
                },
                reason: supported.as_ref().err().cloned(),
                runtime_guard_eligible: true,
            });
        }
    }
    Inventory {
        revision: revision.into(),
        obligations,
    }
}
/// Enrich the syntactic inventory with solver-detected inconsistent premises.
pub fn inventory_verified(program: &Program, revision: &str, solver: &Solver) -> Inventory {
    let mut inventory = inventory(program, revision);
    let targets: std::collections::BTreeSet<_> =
        inventory.obligations.iter().map(|o| o.target).collect();
    for target in targets {
        let report = verification::verify(program, SymbolId(target), solver);
        if report.precondition_consistency.status == Status::Refuted {
            for obligation in inventory
                .obligations
                .iter_mut()
                .filter(|o| o.target == target)
            {
                obligation.support = Support::Inconsistent;
                obligation.reason =
                    Some("preconditions are unsatisfiable; vacuous proof is rejected".into());
            }
        }
    }
    inventory
}

#[derive(Clone, Debug)]
pub struct Example {
    pub name: String,
    pub function: SymbolId,
    pub arguments: Vec<Value>,
    pub expected: Result<Value, TrapKind>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub value: Option<String>,
    pub trap: Option<String>,
    pub cause: Option<String>,
}
impl Outcome {
    pub fn from_result(result: &Result<Value, Trap>) -> Self {
        match result {
            Ok(value) => Self {
                value: Some(format!("{value:?}")),
                trap: None,
                cause: None,
            },
            Err(trap) => Self {
                value: None,
                trap: Some(format!("{:?}", trap.kind)),
                cause: trap.cause.as_ref().map(|t| format!("{:?}", t.kind)),
            },
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestCaseResult {
    pub name: String,
    pub arguments: Vec<String>,
    pub passed: bool,
    pub interpreter: Outcome,
    pub compiled: Option<Outcome>,
    pub failure: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestReport {
    pub program_hash: String,
    pub seed: Option<u64>,
    pub requested: usize,
    pub executed: usize,
    pub discarded_preconditions: usize,
    pub environment: String,
    pub compiled_engine: Option<String>,
    pub cases: Vec<TestCaseResult>,
}
impl TestReport {
    pub fn passed(&self) -> bool {
        self.executed == self.requested && self.cases.iter().all(|c| c.passed)
    }
}
fn report(
    program: &Program,
    seed: Option<u64>,
    requested: usize,
    compiled_engine: Option<&str>,
) -> TestReport {
    TestReport {
        program_hash: verification::program_fingerprint(program),
        seed,
        requested,
        executed: 0,
        discarded_preconditions: 0,
        environment: format!(
            "pactern/{} {}-{}",
            crate::TOOL_VERSION,
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
        compiled_engine: compiled_engine.map(str::to_owned),
        cases: vec![],
    }
}
/// Run explicit examples in both engines. The callback must execute the actual
/// compiled artifact; its engine label is included in the report.
pub fn run_examples<F>(
    program: &Program,
    examples: &[Example],
    limits: Limits,
    compiled_engine: &str,
    mut compiled: F,
) -> TestReport
where
    F: FnMut(SymbolId, Vec<Value>) -> Result<Outcome, String>,
{
    let mut report = report(program, None, examples.len(), Some(compiled_engine));
    for example in examples {
        let result = interpreter::execute_guarded(
            program,
            example.function,
            example.arguments.clone(),
            limits,
        );
        let outcome = Outcome::from_result(&result);
        let expected = match (&example.expected, &result) {
            (Ok(a), Ok(b)) => a == b,
            (Err(a), Err(b)) => *a == b.kind,
            _ => false,
        };
        let compiled = compiled(example.function, example.arguments.clone());
        let equivalent = compiled.as_ref().is_ok_and(|compiled| compiled == &outcome);
        report.cases.push(TestCaseResult {
            name: example.name.clone(),
            arguments: example.arguments.iter().map(|v| format!("{v:?}")).collect(),
            passed: expected && equivalent,
            interpreter: outcome,
            compiled: compiled.as_ref().ok().cloned(),
            failure: if !expected {
                Some("interpreter result differs from expected example".into())
            } else if !equivalent {
                Some(
                    compiled
                        .err()
                        .unwrap_or_else(|| "compiled/interpreter mismatch".into()),
                )
            } else {
                None
            },
        });
        report.executed += 1;
    }
    report
}
/// Seeded I64/Bool property execution checks guards and interpreter/compiled
/// agreement. It is testing, never a proof. False preconditions are discarded;
/// evaluation failures and postcondition failures are recorded as failures.
pub fn run_properties<F>(
    program: &Program,
    function: SymbolId,
    seed: u64,
    cases: usize,
    limits: Limits,
    compiled_engine: &str,
    mut compiled: F,
) -> Result<TestReport, String>
where
    F: FnMut(SymbolId, Vec<Value>) -> Result<Outcome, String>,
{
    if cases == 0 || cases > 10_000 {
        return Err("property case count must be between 1 and 10,000".into());
    }
    let declaration = program
        .declarations()
        .iter()
        .find(|d| d.symbol == function)
        .ok_or("unknown function")?;
    let DeclarationKind::Function {
        type_parameters,
        parameters,
        ..
    } = &declaration.kind
    else {
        return Err("property target is not a function".into());
    };
    if !type_parameters.is_empty() {
        return Err("generic property entry is unsupported".into());
    }
    let types = parameters
        .iter()
        .map(|id| {
            program
                .symbol_types()
                .get(id)
                .ok_or("missing parameter type")
        })
        .collect::<Result<Vec<_>, _>>()?;
    if types.iter().any(|t| {
        !matches!(
            t,
            crate::typing::ValueType::I64 | crate::typing::ValueType::Bool
        )
    }) {
        return Err("seeded generators currently support I64 and Bool arguments".into());
    }
    let mut output = report(program, Some(seed), cases, Some(compiled_engine));
    let mut state = seed;
    let edges = [i64::MIN, i64::MAX, -1, 0, 1, 2, 3, 10];
    for attempt in 0..cases.saturating_mul(20).saturating_add(100) {
        if output.executed == cases {
            break;
        }
        let args: Vec<_> = types
            .iter()
            .enumerate()
            .map(|(index, ty)| {
                state = state.wrapping_add(0x9e3779b97f4a7c15);
                let mut z = state;
                z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
                z ^= z >> 31;
                if **ty == crate::typing::ValueType::Bool {
                    Value::Bool(z & 1 == 1)
                } else {
                    Value::I64(if attempt < edges.len() * edges.len() {
                        edges[(attempt / edges.len().pow(index.min(1) as u32)) % edges.len()]
                    } else {
                        z as i64
                    })
                }
            })
            .collect();
        let result = interpreter::execute_guarded(program, function, args.clone(), limits);
        if result
            .as_ref()
            .is_err_and(|t| t.kind == TrapKind::Precondition)
        {
            output.discarded_preconditions += 1;
            continue;
        }
        let outcome = Outcome::from_result(&result);
        let compiled = compiled(function, args.clone());
        let equivalent = compiled.as_ref().is_ok_and(|c| c == &outcome);
        let passed = result.is_ok() && equivalent;
        output.cases.push(TestCaseResult {
            name: format!("seed-{seed}-attempt-{attempt}"),
            arguments: args.iter().map(|v| format!("{v:?}")).collect(),
            passed,
            interpreter: outcome,
            compiled: compiled.as_ref().ok().cloned(),
            failure: if result.is_err() {
                Some("property execution trapped".into())
            } else if !equivalent {
                Some(
                    compiled
                        .err()
                        .unwrap_or_else(|| "compiled/interpreter mismatch".into()),
                )
            } else {
                None
            },
        });
        output.executed += 1;
    }
    Ok(output)
}

#[derive(Clone, Debug)]
pub struct GuardedArtifact {
    pub c_source: String,
    pub wasm: Vec<u8>,
    pub guards: Vec<crate::evidence::GuardEvidence>,
}
/// Compile all inventoried checks and bind their map to the emitted module.
/// Guards are retained even for statically proved clauses.
pub fn compile_guarded(
    program: &Program,
    revision: &str,
    function: SymbolId,
    arguments: &[Value],
    limits: Limits,
) -> Result<GuardedArtifact, crate::backend::BackendError> {
    let c_source = crate::backend::emit_c_guarded(program, function, arguments, limits)?;
    guarded_artifact(program, revision, function, c_source)
}
/// Compile a terminal entry with the same artifact-bound runtime guard evidence.
pub fn compile_terminal_guarded(
    program: &Program,
    revision: &str,
    function: SymbolId,
    limits: Limits,
) -> Result<GuardedArtifact, crate::backend::BackendError> {
    let c_source = crate::backend::emit_c_terminal(program, function, limits, true)?;
    guarded_artifact(program, revision, function, c_source)
}
fn guarded_artifact(
    program: &Program,
    revision: &str,
    function: SymbolId,
    c_source: String,
) -> Result<GuardedArtifact, crate::backend::BackendError> {
    let wasm = crate::backend::compile_wasm(&c_source)?;
    let artifact_hash = crate::evidence::hash(&wasm);
    let reachable = reachable_functions(program, function);
    let guards = inventory(program, revision)
        .obligations
        .into_iter()
        .filter(|obligation| reachable.contains(&SymbolId(obligation.target)))
        .map(|obligation| crate::evidence::GuardEvidence {
            obligation_id: obligation.id,
            artifact_hash: artifact_hash.clone(),
            phase: obligation.phase,
            function: obligation.target,
            clause: obligation.clause,
        })
        .collect();
    Ok(GuardedArtifact {
        c_source,
        wasm,
        guards,
    })
}
/// Normalize a compiled result for the shared deterministic runner.
pub fn compiled_outcome(execution: crate::backend::Execution) -> Outcome {
    match execution {
        crate::backend::Execution::Value(value) => Outcome {
            value: Some(format!("{value:?}")),
            trap: None,
            cause: None,
        },
        crate::backend::Execution::Trap { kind, cause } => Outcome {
            value: None,
            trap: Some(format!("{kind:?}")),
            cause: cause.map(|c| format!("{c:?}")),
        },
    }
}

fn reachable_functions(program: &Program, entry: SymbolId) -> std::collections::BTreeSet<SymbolId> {
    let mut reachable = std::collections::BTreeSet::new();
    let mut pending = vec![entry];
    while let Some(function) = pending.pop() {
        if !reachable.insert(function) {
            continue;
        }
        for declaration in program.declarations() {
            let relevant = declaration.symbol == function
                || matches!(&declaration.kind, DeclarationKind::Pact { target, .. } if *target == function);
            if !relevant {
                continue;
            }
            for reference in &program.resolution().references {
                if reference.location.file == declaration.origin.file
                    && reference.location.span.start.byte >= declaration.origin.span.start.byte
                    && reference.location.span.end.byte <= declaration.origin.span.end.byte
                    && program.resolution().symbols[reference.symbol.0].kind
                        == crate::resolution::SymbolKind::Function
                {
                    pending.push(reference.symbol);
                }
            }
        }
    }
    reachable
}
