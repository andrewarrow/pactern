//! Scripted semantic repair, protected acceptance, compiled quota rule, and replay.
//! Run: cargo run -p pactern --example quota_workflow -- target/quota-workflow
use pactern::{
    acceptance::{HashRecord, TrustedBaseline},
    backend::{self, Execution, HostLimits},
    core::{self, Program},
    evidence::{self, Binding, BindingInputs, Evidence, Policy},
    host::{Budget, Grant, HostError, Invocation, Operation as HostOperation, QuotaStore},
    interpreter::{Limits, TrapKind, Value},
    pacts::{self, Example},
    resolution::{ParsedFile, SymbolId, SymbolKind},
    semantic::{SemanticError, SemanticIndex, TestDependency},
    syntax::{SourceFile, parse},
    verification::{self, Solver, Status},
    workspace::{
        self, ChangeRequest, Operation, PactTarget, SymbolTarget, Transaction, Workspace,
        WorkspaceData, WorkspaceSnapshot,
    },
};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const BROKEN: &str = include_str!("../../../fixtures/conformance/variants/quota_unchecked.ptn");
const REPAIRED: &str = include_str!("../../../fixtures/conformance/src/quota.ptn");
const PACT: &str = include_str!("../../../fixtures/conformance/contracts/quota.pact.ptn");
fn lower(files: &BTreeMap<String, Vec<u8>>) -> Result<Program> {
    let snapshot = workspace::source_snapshot(files)?;
    let parsed = snapshot
        .inputs()
        .iter()
        .map(|input| {
            let source = SourceFile::from_bytes(&input.path, &input.bytes)?;
            Ok(ParsedFile::new(&input.path, parse(&source, input.kind)?))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(core::lower(&parsed)?)
}
fn function(program: &Program) -> SymbolId {
    program
        .resolution()
        .symbols
        .iter()
        .find(|s| s.kind == SymbolKind::Function && s.name == "reserve")
        .expect("quota entry")
        .id
}
fn executable(path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    if path.is_file() {
        return Ok(path.canonicalize()?);
    }
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let candidate = directory.join(path);
        if candidate.is_file() {
            return Ok(candidate.canonicalize()?);
        }
    }
    Err(format!("required executable is unavailable: {}", path.display()).into())
}
fn proved(report: &verification::Report) -> bool {
    [
        &report.precondition_consistency,
        &report.postcondition,
        &report.absence_of_traps,
        &report.termination,
    ]
    .iter()
    .all(|claim| claim.status == Status::Proved)
}
fn check_candidate(
    snapshot: &WorkspaceSnapshot,
) -> std::result::Result<BTreeSet<String>, SemanticError> {
    let program =
        lower(&snapshot.files).map_err(|e| SemanticError::new("PTN_DEMO_CHECK", e.to_string()))?;
    let report = verification::verify(&program, function(&program), &Solver::default());
    if !proved(&report) {
        return Err(SemanticError::new(
            "PTN_DEMO_PROOF_REQUIRED",
            "quota candidate lacks all four required proofs",
        ));
    }
    Ok(BTreeSet::from(["quota-proof".into()]))
}
fn repair_request(snapshot: &WorkspaceSnapshot, request_id: &str) -> Result<ChangeRequest> {
    let target = snapshot.index.find("reserve")?;
    let function_start = REPAIRED
        .find("pub fn reserve")
        .ok_or("repair function missing")?;
    let body_start = REPAIRED[function_start..]
        .find('{')
        .ok_or("repair body missing")?
        + function_start;
    Ok(ChangeRequest {
        protocol_version: "0.1.0".into(),
        kind: "change_request".into(),
        request_id: request_id.into(),
        transaction: Transaction::Implementation,
        base_revision: snapshot.revision.clone(),
        expected_policy_baseline: snapshot.policy_baseline.clone(),
        operations: vec![Operation::ReplaceFunctionBody {
            target: SymbolTarget {
                symbol_id: target.id.clone(),
                expected_hash: target.declaration_hash.clone(),
            },
            body: REPAIRED[body_start..].trim().into(),
        }],
    })
}
fn grant() -> Vec<Grant> {
    vec![Grant {
        tenant: "tenant-a".into(),
        operations: BTreeSet::from([HostOperation::Read, HostOperation::Write]),
    }]
}
fn remaining(program: &Program, outcome: Execution) -> Result<i64> {
    match outcome {
        Execution::Value(Value::Variant { variant, payload })
            if program.resolution().symbols[variant.0].name == "Ok" =>
        {
            match payload.as_slice() {
                [Value::I64(value)] => Ok(*value),
                _ => Err("invalid quota result".into()),
            }
        }
        _ => Err("expected successful quota result".into()),
    }
}
fn save_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn run() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/quota-workflow"));
    std::fs::create_dir_all(&output)?;
    let compiler_path = std::env::current_exe()?.canonicalize()?;
    let compiler_bytes = std::fs::read(&compiler_path)?;
    let solver_path = executable("z3")?;
    let solver_bytes = std::fs::read(&solver_path)?;
    let clang_path = executable(backend::compiler_path())?;
    let linker_path = executable(backend::linker_path())?;
    let mut dependencies: BTreeMap<String, Vec<u8>> = BTreeMap::from([
        ("tools/clang".into(), std::fs::read(&clang_path)?),
        ("tools/wasm-ld".into(), std::fs::read(&linker_path)?),
        (
            "runtime/pactern.c".into(),
            std::fs::read(root.join("runtime/pactern.c"))?,
        ),
        ("Cargo.lock".into(), std::fs::read(root.join("Cargo.lock"))?),
        (
            "rust-toolchain.toml".into(),
            std::fs::read(root.join("rust-toolchain.toml"))?,
        ),
    ]);
    let mut external: Vec<_> = dependencies
        .iter()
        .map(|(key, bytes)| HashRecord {
            category: "dependency".into(),
            key: key.clone(),
            digest: evidence::hash(bytes),
        })
        .collect();
    external.push(HashRecord {
        category: "tool".into(),
        key: "pactern-compiler-host".into(),
        digest: evidence::hash(&compiler_bytes),
    });
    external.push(HashRecord {
        category: "tool".into(),
        key: "z3".into(),
        digest: evidence::hash(&solver_bytes),
    });
    let files = BTreeMap::from([
        ("src/quota.ptn".into(), BROKEN.as_bytes().to_vec()),
        ("contracts/quota.pact.ptn".into(), PACT.as_bytes().to_vec()),
    ]);
    let data = WorkspaceData::new(files, external.clone())?;
    let index = SemanticIndex::build(&workspace::source_snapshot(&data.files)?, &data.ledger)?;
    let mut baseline = TrustedBaseline::capture(
        &data.files,
        &index,
        &["contracts/quota.pact.ptn".into()],
        &[],
        external,
    )?;
    baseline.require_check("quota-proof");
    let workspace = Workspace::new(data.clone(), baseline.clone())?;
    let before = workspace.snapshot()?;
    let context = before.index.inspect("reserve")?;
    let impact = before.index.impact(&["reserve".into()])?;
    let affected = before.index.affected_tests(
        &["reserve".into()],
        &[TestDependency {
            test: "quota-examples".into(),
            declarations: BTreeSet::from([context.result.id.clone()]),
            complete: true,
        }],
    )?;
    let broken = lower(&before.files)?;
    let refutation = verification::verify(&broken, function(&broken), &Solver::default());
    if refutation.postcondition.status != Status::Refuted {
        return Err("expected a real SMT refutation of unchecked quota".into());
    }

    // Both direct-text and semantic attempts to erase the protected requirement fail.
    let mut weakened = data.files.clone();
    weakened.insert(
        "contracts/quota.pact.ptn".into(),
        b"module quota; pact reserve_safety for reserve { ensures true; }".to_vec(),
    );
    let weakened_index =
        SemanticIndex::build(&workspace::source_snapshot(&weakened)?, &data.ledger)?;
    let text_rejection = baseline
        .check(&weakened, &weakened_index)
        .err()
        .ok_or("protected text edit unexpectedly passed")?;
    let pact = before.index.find("reserve_safety")?;
    let weakening = ChangeRequest {
        protocol_version: "0.1.0".into(),
        kind: "change_request".into(),
        request_id: "weaken-attempt".into(),
        transaction: Transaction::Implementation,
        base_revision: before.revision.clone(),
        expected_policy_baseline: before.policy_baseline.clone(),
        operations: vec![Operation::ReplacePact {
            target: PactTarget {
                pact_id: pact.id.clone(),
                expected_hash: pact.declaration_hash.clone(),
            },
            declaration: "pact reserve_safety for reserve { ensures true; }".into(),
        }],
    };
    let semantic_rejection = workspace
        .stage(&weakening)
        .err()
        .ok_or("protected semantic edit unexpectedly passed")?;
    if workspace.snapshot()?.revision != before.revision {
        return Err("rejected weakening changed the workspace".into());
    }

    // The trusted Rust host calls the pure compiled rule inside its transaction.
    let store = Arc::new(QuotaStore::new([
        ("tenant-a".into(), 3),
        ("tenant-b".into(), 9),
    ])?);
    let mut failed_invocation = Invocation::record(
        store.clone(),
        grant(),
        Budget::default(),
        &before.revision,
        &before.policy_baseline,
    );
    let failed_handle = failed_invocation.begin("tenant-a")?;
    let available = failed_invocation.read(&failed_handle)?;
    let failed_artifact = pacts::compile_guarded(
        &broken,
        &before.revision,
        function(&broken),
        &[Value::I64(available), Value::I64(4)],
        Limits::default(),
    )?;
    let failed_result = backend::run_wasm(&failed_artifact.wasm, HostLimits::default())?;
    if !matches!(
        failed_result,
        Execution::Trap {
            kind: TrapKind::Postcondition,
            ..
        }
    ) {
        return Err("defective quota failed to trigger its guard".into());
    }
    failed_invocation.rollback(failed_handle)?;
    let failed_trace = failed_invocation.finish()?;
    if store.live_writes() != 0 {
        return Err("guard failure wrote to storage".into());
    }

    let repaired_request = repair_request(&before, "repair-quota")?;
    let candidate = workspace.stage_with(&repaired_request, check_candidate)?;
    let stale = workspace.stage_with(
        &repair_request(&before, "concurrent-repair")?,
        check_candidate,
    )?;
    let receipt = workspace.commit(candidate, None)?;
    let stale_rejection = workspace
        .commit(stale, None)
        .err()
        .ok_or("stale candidate unexpectedly committed")?;
    let after = workspace.snapshot()?;
    if after.index.find("reserve")?.id != context.result.id {
        return Err("repair lost persistent identity".into());
    }
    baseline.check(&after.files, &after.index)?;
    let repaired = lower(&after.files)?;
    let proof = verification::verify(&repaired, function(&repaired), &Solver::default());
    if !proved(&proof) {
        return Err("repaired quota proof failed".into());
    }

    let mut invocation = Invocation::record(
        store.clone(),
        grant(),
        Budget::default(),
        &after.revision,
        &after.policy_baseline,
    );
    if invocation.begin("tenant-b").err() != Some(HostError::Denied) {
        return Err("cross-tenant access was not denied".into());
    }
    let handle = invocation.begin("tenant-a")?;
    let available = invocation.read(&handle)?;
    let artifact = pacts::compile_guarded(
        &repaired,
        &after.revision,
        function(&repaired),
        &[Value::I64(available), Value::I64(2)],
        Limits::default(),
    )?;
    let expected_balance = remaining(
        &repaired,
        backend::run_wasm(&artifact.wasm, HostLimits::default())?,
    )?;
    if !invocation.reserve(&handle, 2)? || invocation.read(&handle)? != expected_balance {
        return Err("compiled quota rule and transaction disagree".into());
    }
    invocation.commit(handle)?;
    let success_trace = invocation.finish()?;
    if store.balance("tenant-a")? != 1 || store.balance("tenant-b")? != 9 {
        return Err("transaction violated tenant balances".into());
    }

    // Replay the original failure against recorded old revision, never live I/O.
    let writes_before_replay = store.live_writes();
    let mut replay = Invocation::replay(
        store.clone(),
        grant(),
        Budget::default(),
        &before.revision,
        &before.policy_baseline,
        failed_trace.clone(),
    )?;
    let handle = replay.begin("tenant-a")?;
    if replay.read(&handle)? != 3 {
        return Err("replay used current storage instead of its recording".into());
    }
    if backend::run_wasm(&failed_artifact.wasm, HostLimits::default())? != failed_result {
        return Err("failure replay differs".into());
    }
    replay.rollback(handle)?;
    replay.finish()?;
    if store.live_writes() != writes_before_replay {
        return Err("replay performed live writes".into());
    }
    let revision_mismatch = Invocation::replay(
        store.clone(),
        grant(),
        Budget::default(),
        &after.revision,
        &before.policy_baseline,
        failed_trace.clone(),
    )
    .err()
    .ok_or("replay accepted wrong revision")?;

    let function = function(&repaired);
    let ok = repaired
        .resolution()
        .symbols
        .iter()
        .find(|s| s.kind == SymbolKind::BuiltinVariant && s.name == "Ok")
        .ok_or("Ok constructor missing")?
        .id;
    let examples = [
        Example {
            name: "quota-success".into(),
            function,
            arguments: vec![Value::I64(3), Value::I64(2)],
            expected: Ok(Value::Variant {
                variant: ok,
                payload: vec![Value::I64(1)],
            }),
        },
        Example {
            name: "quota-precondition".into(),
            function,
            arguments: vec![Value::I64(-1), Value::I64(1)],
            expected: Err(TrapKind::Precondition),
        },
    ];
    let mut compiled_count = 0;
    let mut compiled = |entry, arguments: Vec<Value>| {
        let c = backend::emit_c_guarded(&repaired, entry, &arguments, Limits::default())
            .map_err(|e| e.to_string())?;
        let wasm = backend::compile_wasm(&c).map_err(|e| e.to_string())?;
        let outcome = backend::run_wasm(&wasm, HostLimits::default()).map_err(|e| e.to_string())?;
        dependencies.insert(
            format!("executed-test-artifacts/{compiled_count}.wasm"),
            wasm,
        );
        compiled_count += 1;
        Ok(pacts::compiled_outcome(outcome))
    };
    let examples = pacts::run_examples(
        &repaired,
        &examples,
        Limits::default(),
        "wasmi/2.0.0-guarded",
        &mut compiled,
    );
    let properties = pacts::run_properties(
        &repaired,
        function,
        0x50414354,
        24,
        Limits::default(),
        "wasmi/2.0.0-guarded",
        &mut compiled,
    )?;
    let mut policy = Policy::default();
    policy.required_proof_functions.insert(function.0);
    policy
        .required_guards
        .extend(artifact.guards.iter().map(|g| g.obligation_id.clone()));
    policy
        .required_tests
        .extend(["quota-examples".into(), "quota-properties".into()]);
    dependencies.insert(
        "trusted-baseline.json".into(),
        serde_json::to_vec(&baseline)?,
    );
    dependencies.insert("host-grants.json".into(), serde_json::to_vec(&grant())?);
    dependencies.insert(
        "host-budget.json".into(),
        serde_json::to_vec(&Budget::default())?,
    );
    let mut assumptions = proof.assumptions.clone();
    assumptions.push("Trusted Rust host invokes the pure compiled rule inside an optimistic tenant transaction; this workflow does not use guest host imports".into());
    let binding = Binding::new(BindingInputs {
        program: &repaired,
        sources: &after.files,
        dependencies: &dependencies,
        compiler_bytes: &compiler_bytes,
        solver_version: &proof.solver,
        solver_bytes: &solver_bytes,
        target: "wasm32-unknown-unknown/invocation-abi-v1",
        artifact: &artifact.wasm,
        policy: &policy,
        assumptions,
    });
    let evidence = Evidence {
        schema_version: "0.1.0".into(),
        binding: binding.clone(),
        proofs: vec![proof],
        guards: artifact.guards,
        tests: BTreeMap::from([
            ("quota-examples".into(), examples),
            ("quota-properties".into(), properties),
        ]),
    };
    let acceptance = evidence.evaluate(&binding, &policy);
    if !acceptance.accepted {
        return Err(format!("evidence rejected: {:?}", acceptance.reasons).into());
    }

    std::fs::write(output.join("quota.wasm"), &artifact.wasm)?;
    std::fs::write(output.join("quota.c"), &artifact.c_source)?;
    std::fs::write(output.join("failure.wasm"), &failed_artifact.wasm)?;
    save_json(&output.join("evidence.json"), &evidence)?;
    save_json(&output.join("policy.json"), &policy)?;
    save_json(&output.join("trusted-baseline.json"), &baseline)?;
    save_json(&output.join("failure-trace.json"), &failed_trace)?;
    save_json(&output.join("success-trace.json"), &success_trace)?;
    save_json(&output.join("refutation.json"), &refutation)?;
    save_json(&output.join("repair-request.json"), &repaired_request)?;
    save_json(&output.join("workspace.json"), &after.files)?;
    let summary = json!({"kind":"quota_workflow","scripted_not_agent_benchmark":true,"context":context,"impact":impact,"affected_tests":affected,"defect_refuted":true,"protected_text_rejection":text_rejection,"protected_semantic_rejection":semantic_rejection,"repair":receipt,"stale_rejection":stale_rejection,"failure_guard":"Postcondition","tenant_balances":{"tenant-a":store.balance("tenant-a")?,"tenant-b":store.balance("tenant-b")?},"replay_live_writes":store.live_writes()-writes_before_replay,"replay_revision_mismatch":revision_mismatch,"acceptance":acceptance,"evidence_revision":binding.revision(),"artifact_hash":binding.artifact_hash,"tools":{"pactern_host":compiler_path,"clang":clang_path,"wasm_ld":linker_path,"z3":solver_path},"output":output});
    save_json(&output.join("workflow.json"), &summary)?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"accepted":true,"output":output,"artifact_hash":binding.artifact_hash,"replay_live_writes":0})
        )?
    );
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("quota workflow failed: {error}");
        std::process::exit(1);
    }
}
