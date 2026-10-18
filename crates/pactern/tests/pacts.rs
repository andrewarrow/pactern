use pactern::{
    core::{self, Program},
    interpreter::{self, Limits, TrapKind, Value},
    resolution::{ParsedFile, SymbolId, SymbolKind},
    syntax::{FileKind, SourceFile, parse},
    verification::{self, Solver, Status},
};
fn load(source: &str, contract: &str) -> Program {
    core::lower(&[
        ParsedFile::new(
            "src/main.ptn",
            parse(
                &SourceFile::new("src/main.ptn", source),
                FileKind::Implementation,
            )
            .unwrap(),
        ),
        ParsedFile::new(
            "contracts/main.pact.ptn",
            parse(
                &SourceFile::new("contracts/main.pact.ptn", contract),
                FileKind::Contract,
            )
            .unwrap(),
        ),
    ])
    .unwrap()
}
fn fixture(source: &str, contract: &str) -> Program {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/conformance");
    let src = std::fs::read_to_string(root.join(source)).unwrap();
    let pact = std::fs::read_to_string(root.join(contract)).unwrap();
    let module = src
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("module ")
        .unwrap();
    load(
        &src.replacen(&format!("module {module};"), "module main;", 1),
        &pact.replacen(&format!("module {module};"), "module main;", 1),
    )
}
fn id(p: &Program, name: &str) -> SymbolId {
    p.resolution()
        .symbols
        .iter()
        .find(|s| s.name == name && s.kind == SymbolKind::Function)
        .unwrap()
        .id
}
fn check(p: &Program, name: &str) -> verification::Report {
    verification::verify(p, id(p, name), &Solver::default())
}
fn has_solver() -> bool {
    std::process::Command::new("z3")
        .arg("-version")
        .output()
        .is_ok()
}
#[test]
fn every_recorded_contract_guard() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/conformance");
    let corpus: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("manifest.json")).unwrap())
            .unwrap();
    let mut count = 0;
    for case in corpus["cases"].as_array().unwrap() {
        if case["pacts"].is_null() {
            continue;
        }
        let p = fixture(
            case["source"].as_str().unwrap(),
            case["contracts"][0].as_str().unwrap(),
        );
        let entry = case["entry"].as_str().unwrap().rsplit("::").next().unwrap();
        let args: Vec<_> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| Value::I64(v.as_str().unwrap().parse().unwrap()))
            .collect();
        let result =
            interpreter::execute_guarded(&p, id(&p, entry), args.clone(), Limits::default());
        let c =
            pactern::backend::emit_c_guarded(&p, id(&p, entry), &args, Limits::default()).unwrap();
        let wasm = pactern::backend::compile_wasm(&c).unwrap();
        let compiled =
            pactern::backend::run_wasm(&wasm, pactern::backend::HostLimits::default()).unwrap();
        let expected = match &result {
            Ok(v) => pactern::backend::Execution::Value(v.clone()),
            Err(t) => pactern::backend::Execution::Trap {
                kind: t.kind,
                cause: t.cause.as_ref().map(|t| t.kind),
            },
        };
        assert_eq!(compiled, expected, "{}", case["id"]);
        match case["expected"]["category"].as_str() {
            Some(category) => {
                let trap = result.unwrap_err();
                assert_eq!(
                    trap.kind,
                    match category {
                        "precondition" => TrapKind::Precondition,
                        "postcondition" => TrapKind::Postcondition,
                        "contract_evaluation" => TrapKind::ContractEvaluation,
                        x => panic!("{x}"),
                    },
                    "{}",
                    case["id"]
                );
                if case["expected"]["cause"] == "division_by_zero" {
                    assert_eq!(trap.cause.unwrap().kind, TrapKind::DivisionByZero);
                }
            }
            None => assert!(result.is_ok(), "{}: {result:?}", case["id"]),
        }
        count += 1;
    }
    assert_eq!(count, 17);
}
#[test]
fn internal_calls_check_pacts_and_body_traps_do_not_check_postconditions() {
    let p = load(
        "module main; fn helper(x:I64)->I64{return x;} fn run()->I64{return helper(0);}",
        "module main; pact p for helper { requires x>0; ensures result==x; }",
    );
    assert_eq!(
        interpreter::execute_guarded(&p, id(&p, "run"), vec![], Limits::default())
            .unwrap_err()
            .kind,
        TrapKind::Precondition
    );
    let p = load(
        "module main; fn run()->I64{return 1/0;}",
        "module main; pact p for run { ensures false; }",
    );
    assert_eq!(
        interpreter::execute_guarded(&p, id(&p, "run"), vec![], Limits::default())
            .unwrap_err()
            .kind,
        TrapKind::DivisionByZero
    );
}
#[test]
fn quota_has_real_unbounded_proofs_and_variants_are_refuted() {
    if !has_solver() {
        eprintln!("z3 unavailable; external solver checks skipped");
        return;
    }
    let p = fixture("src/quota.ptn", "contracts/quota.pact.ptn");
    let report = check(&p, "reserve");
    assert_eq!(
        report.precondition_consistency.status,
        Status::Proved,
        "{report:?}"
    );
    assert_eq!(report.postcondition.status, Status::Proved, "{report:?}");
    assert_eq!(report.absence_of_traps.status, Status::Proved, "{report:?}");
    assert_eq!(report.termination.status, Status::Proved);
    assert_eq!(report.exploration_bound, None);
    for source in [
        "variants/quota_unchecked.ptn",
        "variants/quota_reject_all.ptn",
    ] {
        let p = fixture(source, "contracts/quota.pact.ptn");
        let report = check(&p, "reserve");
        assert_eq!(report.postcondition.status, Status::Refuted, "{report:?}");
    }
}
#[test]
fn recorded_verification_cases() {
    if !has_solver() {
        return;
    }
    for (name, function, status) in [
        ("verification_proved", "identity", Status::Proved),
        ("verification_refuted", "positive", Status::Refuted),
        (
            "verification_unsupported",
            "count_down",
            Status::Unsupported,
        ),
    ] {
        let p = fixture(
            &format!("src/{name}.ptn"),
            &format!("contracts/{name}.pact.ptn"),
        );
        assert_eq!(check(&p, function).postcondition.status, status);
    }
    let p = fixture(
        "src/contract_inconsistent.ptn",
        "contracts/contract_inconsistent.pact.ptn",
    );
    let report = check(&p, "run");
    assert_eq!(report.precondition_consistency.status, Status::Refuted);
    assert_eq!(report.postcondition.status, Status::Unknown);
}
#[test]
fn machine_integer_traps_are_separate_from_postconditions() {
    if !has_solver() {
        return;
    }
    for expression in ["x+1", "x-1", "x*2", "-x", "x/-1", "x%-1", "1/x"] {
        let p = load(
            &format!("module main; fn run(x:I64)->I64{{return {expression};}}"),
            "module main; pact p for run { ensures true; }",
        );
        let report = check(&p, "run");
        assert_eq!(
            report.postcondition.status,
            Status::Proved,
            "{expression}: {report:?}"
        );
        assert_eq!(
            report.absence_of_traps.status,
            Status::Refuted,
            "{expression}: {report:?}"
        );
    }
    let p = load(
        "module main; fn run(x:I64)->I64{return x+1;}",
        "module main; pact p for run { requires x<9223372036854775807; ensures result>x; }",
    );
    let report = check(&p, "run");
    assert_eq!(report.absence_of_traps.status, Status::Proved, "{report:?}");
    assert_eq!(report.postcondition.status, Status::Proved);
}
#[test]
fn short_circuit_and_eager_unused_traps_are_exact() {
    if !has_solver() {
        return;
    }
    let p = load(
        "module main; fn run(x:I64)->Bool{return x!=0 && 1/x>0;}",
        "module main; pact p for run { ensures true; }",
    );
    assert_eq!(check(&p, "run").absence_of_traps.status, Status::Proved);
    let p = load(
        "module main; fn run(x:I64)->I64{let unused=1/x; return 0;}",
        "module main; pact p for run { ensures result==0; }",
    );
    assert_eq!(check(&p, "run").absence_of_traps.status, Status::Refuted);
    let p = load(
        "module main; fn run(x:I64)->I64{return x;}",
        "module main; pact p for run { requires 1/x>0; ensures true; }",
    );
    assert_eq!(check(&p, "run").absence_of_traps.status, Status::Refuted);
}
#[test]
fn unavailable_solver_is_unknown() {
    let p = load(
        "module main; fn run()->I64{return 1;}",
        "module main; pact p for run { ensures result==1; }",
    );
    let report = verification::verify(
        &p,
        id(&p, "run"),
        &Solver {
            executable: "/does/not/exist".into(),
            timeout_ms: 1,
        },
    );
    assert_eq!(report.postcondition.status, Status::Unknown);
}
#[test]
fn revision_inventory_distinguishes_unsupported_and_inconsistent() {
    use pactern::pacts::{self, Support};
    let p = fixture("src/quota.ptn", "contracts/quota.pact.ptn");
    let a = pacts::inventory(&p, "sha256:first");
    let b = pacts::inventory(&p, "sha256:second");
    assert_eq!(a.obligations.len(), 2);
    assert_ne!(a.obligations[0].id, b.obligations[0].id);
    assert!(
        a.obligations
            .iter()
            .all(|o| matches!(o.support, Support::Supported))
    );
    let p = fixture(
        "src/verification_unsupported.ptn",
        "contracts/verification_unsupported.pact.ptn",
    );
    assert!(matches!(
        pacts::inventory(&p, "revision").obligations[0].support,
        Support::Unsupported
    ));
    if has_solver() {
        let p = fixture(
            "src/contract_inconsistent.ptn",
            "contracts/contract_inconsistent.pact.ptn",
        );
        assert!(
            pacts::inventory_verified(&p, "revision", &Solver::default())
                .obligations
                .iter()
                .all(|o| matches!(o.support, Support::Inconsistent))
        );
    }
}
fn compiled_outcome(
    p: &Program,
    function: SymbolId,
    arguments: Vec<Value>,
) -> Result<pactern::pacts::Outcome, String> {
    let c = pactern::backend::emit_c_guarded(p, function, &arguments, Limits::default())
        .map_err(|e| e.to_string())?;
    let wasm = pactern::backend::compile_wasm(&c).map_err(|e| e.to_string())?;
    let result = pactern::backend::run_wasm(&wasm, pactern::backend::HostLimits::default())
        .map_err(|e| e.to_string())?;
    Ok(pactern::pacts::compiled_outcome(result))
}
#[test]
fn seeded_properties_are_reproducible_and_detect_contract_failures() {
    use pactern::pacts;
    let p = fixture("src/quota.ptn", "contracts/quota.pact.ptn");
    let function = id(&p, "reserve");
    let mut cache: std::collections::BTreeMap<String, pacts::Outcome> =
        std::collections::BTreeMap::new();
    let mut compiled = |function, arguments: Vec<Value>| {
        let key = format!("{arguments:?}");
        if let Some(value) = cache.get(&key) {
            return Ok(value.clone());
        }
        let value = compiled_outcome(&p, function, arguments)?;
        cache.insert(key, value.clone());
        Ok(value)
    };
    let first = pacts::run_properties(
        &p,
        function,
        42,
        24,
        Limits::default(),
        "wasmi/guarded",
        &mut compiled,
    )
    .unwrap();
    let second = pacts::run_properties(
        &p,
        function,
        42,
        24,
        Limits::default(),
        "wasmi/guarded",
        &mut compiled,
    )
    .unwrap();
    assert!(first.passed(), "{first:?}");
    assert!(first.discarded_preconditions > 0);
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(second).unwrap()
    );
    let p = fixture("variants/quota_reject_all.ptn", "contracts/quota.pact.ptn");
    let failed = pacts::run_properties(
        &p,
        id(&p, "reserve"),
        42,
        24,
        Limits::default(),
        "wasmi/guarded",
        |f, a| compiled_outcome(&p, f, a),
    )
    .unwrap();
    assert!(!failed.passed());
    assert!(
        failed
            .cases
            .iter()
            .any(|c| c.interpreter.trap.as_deref() == Some("Postcondition"))
    );
}
#[test]
fn finite_records_option_and_helper_guards_are_encoded() {
    if !has_solver() {
        return;
    }
    let p = load(
        "module main; struct Pair { first:I64, second:I64 } fn helper(p:Pair)->I64{return p.first;} fn run(x:I64)->Option<I64>{let p=Pair{first:x,second:0};return Option::Some(helper(p));}",
        "module main; pact p for run { ensures match(result){Option::Some(v)=>v==x,Option::None=>false,}; }",
    );
    let report = check(&p, "run");
    assert_eq!(report.postcondition.status, Status::Proved, "{report:?}");
    assert_eq!(report.absence_of_traps.status, Status::Proved);
    let p = load(
        "module main; fn helper(x:I64)->I64{return x;} fn run(x:I64)->I64{return helper(x);}",
        "module main; pact p for helper { requires x>0; } pact q for run { ensures result==x; }",
    );
    assert_eq!(check(&p, "run").absence_of_traps.status, Status::Refuted);
    let p = load(
        "module main; fn run(x:I64)->Unit{let unused=1/x;}",
        "module main; pact q for run { ensures true; }",
    );
    assert_eq!(check(&p, "run").absence_of_traps.status, Status::Refuted);
}
#[test]
fn evidence_requires_exact_bindings_and_keeps_proof_guard_and_test_separate() {
    use pactern::{
        evidence::{self, Binding, BindingInputs, Evidence, Policy},
        pacts::{self, Example},
    };
    use std::collections::BTreeMap;
    let p = load(
        "module main; fn run(x:I64)->I64{return x;}",
        "module main; pact p for run { ensures result==x; }",
    );
    let function = id(&p, "run");
    let mut policy = Policy::default();
    policy.required_proof_functions.insert(function.0);
    policy.required_tests.insert("examples".into());
    let artifact = pacts::compile_guarded(
        &p,
        "source-revision",
        function,
        &[Value::I64(3)],
        Limits::default(),
    )
    .unwrap();
    for guard in &artifact.guards {
        policy.required_guards.insert(guard.obligation_id.clone());
    }
    let tests = pacts::run_examples(
        &p,
        &[Example {
            name: "identity".into(),
            function,
            arguments: vec![Value::I64(3)],
            expected: Ok(Value::I64(3)),
        }],
        Limits::default(),
        "wasmi/guarded",
        |f, a| compiled_outcome(&p, f, a),
    );
    assert!(tests.passed());
    let proof = check(&p, "run");
    let sources = BTreeMap::from([("src/main.ptn".into(), b"exact-source".to_vec())]);
    let dependencies = BTreeMap::from([("runtime/pactern.c".into(), b"runtime".to_vec())]);
    let binding = Binding::new(BindingInputs {
        program: &p,
        sources: &sources,
        dependencies: &dependencies,
        compiler_bytes: b"compiler",
        solver_version: &proof.solver,
        solver_bytes: b"solver",
        target: "wasm32",
        artifact: &artifact.wasm,
        policy: &policy,
        assumptions: proof.assumptions.clone(),
    });
    let mut evidence = Evidence {
        schema_version: "0.1.0".into(),
        binding: binding.clone(),
        proofs: vec![proof],
        guards: artifact.guards,
        tests: BTreeMap::from([("examples".into(), tests)]),
    };
    assert_eq!(evidence.evaluate(&binding, &policy).accepted, has_solver());
    let foreign = load(
        "module main; fn run(x:I64)->I64{return x+0;}",
        "module main; pact p for run { ensures result==x; }",
    );
    let original_hash = evidence.proofs[0].program_hash.clone();
    evidence.proofs[0].program_hash = verification::program_fingerprint(&foreign);
    assert!(
        evidence
            .evaluate(&binding, &policy)
            .reasons
            .iter()
            .any(|reason| reason.contains("another checked program"))
    );
    evidence.proofs[0].program_hash = original_hash;
    evidence.proofs[0]
        .assumptions
        .push("unbound extra premise".into());
    assert!(
        evidence
            .evaluate(&binding, &policy)
            .reasons
            .iter()
            .any(|reason| reason.contains("assumptions absent"))
    );
    evidence.proofs[0].assumptions.pop();
    let mut changed = binding.clone();
    changed.artifact_hash = evidence::hash(b"other artifact");
    assert!(!evidence.evaluate(&changed, &policy).accepted);
    let mut changed = binding.clone();
    changed
        .source_hashes
        .insert("src/main.ptn".into(), evidence::hash(b"changed"));
    assert!(!evidence.reusable_for(&changed));
    let mut changed = binding.clone();
    changed
        .dependency_hashes
        .insert("runtime/pactern.c".into(), evidence::hash(b"changed"));
    assert!(!evidence.reusable_for(&changed));
    let mut changed = binding.clone();
    changed.compiler_hash = evidence::hash(b"new compiler");
    assert!(!evidence.reusable_for(&changed));
    let mut changed = binding.clone();
    changed.solver_hash = evidence::hash(b"new solver");
    assert!(!evidence.reusable_for(&changed));
    let mut changed = binding.clone();
    changed.target = "native".into();
    assert!(!evidence.reusable_for(&changed));
    let mut changed = binding.clone();
    changed.assumptions.push("new assumption".into());
    assert!(!evidence.reusable_for(&changed));
    evidence.proofs[0].postcondition.status = Status::Unknown;
    assert!(
        !evidence.evaluate(&binding, &policy).accepted,
        "guards and tests cannot substitute for proof"
    );
    evidence.proofs[0].postcondition.status = Status::Refuted;
    let mut optional = policy.clone();
    optional.required_proof_functions.clear();
    evidence.binding.policy_hash = Binding::new(BindingInputs {
        program: &p,
        sources: &sources,
        dependencies: &dependencies,
        compiler_bytes: b"compiler",
        solver_version: &evidence.proofs[0].solver,
        solver_bytes: b"solver",
        target: "wasm32",
        artifact: &artifact.wasm,
        policy: &optional,
        assumptions: vec![],
    })
    .policy_hash;
    assert!(
        !evidence.evaluate(&evidence.binding, &optional).accepted,
        "any refuted claim rejects the candidate"
    );
}
#[test]
#[cfg(unix)]
fn nonresponsive_solver_times_out_instead_of_proving() {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::temp_dir().join(format!("pactern-timeout-solver-{}", std::process::id()));
    std::fs::write(&path, "#!/bin/sh\nif [ \"$1\" = \"-version\" ]; then echo timeout-test; exit 0; fi\nwhile :; do :; done\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let p = load(
        "module main; fn run()->I64{return 1;}",
        "module main; pact p for run { ensures result==1; }",
    );
    let started = std::time::Instant::now();
    let report = verification::verify(
        &p,
        id(&p, "run"),
        &Solver {
            executable: path.to_str().unwrap().into(),
            timeout_ms: 1,
        },
    );
    std::fs::remove_file(path).unwrap();
    assert_eq!(report.postcondition.status, Status::Unknown);
    assert_eq!(report.absence_of_traps.status, Status::Unknown);
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}
#[test]
fn artifact_guard_map_excludes_unreachable_functions() {
    let p = load(
        "module main; fn run(x:I64)->I64{return helper(x);} fn helper(x:I64)->I64{return x;} fn unused(x:I64)->I64{return x;}",
        "module main; pact p for run { ensures result==x; } pact q for helper { ensures result==x; } pact r for unused { ensures false; }",
    );
    let artifact = pactern::pacts::compile_guarded(
        &p,
        "revision",
        id(&p, "run"),
        &[Value::I64(2)],
        Limits::default(),
    )
    .unwrap();
    assert_eq!(artifact.guards.len(), 2);
    assert!(
        artifact
            .guards
            .iter()
            .all(|g| g.function != id(&p, "unused").0)
    );
    assert!(
        artifact
            .guards
            .iter()
            .all(|g| g.artifact_hash == pactern::evidence::hash(&artifact.wasm))
    );
}
#[test]
fn nested_contract_evaluation_preserves_the_underlying_cause_in_both_engines() {
    let p = load(
        "module main; fn helper(x:I64)->I64{return x;} fn run(x:I64)->I64{return x;}",
        "module main; pact h for helper { requires 1/x>0; } pact r for run { ensures helper(result)>0; }",
    );
    let result =
        interpreter::execute_guarded(&p, id(&p, "run"), vec![Value::I64(0)], Limits::default());
    let trap = result.as_ref().unwrap_err();
    assert_eq!(trap.kind, TrapKind::ContractEvaluation);
    assert_eq!(trap.cause.as_ref().unwrap().kind, TrapKind::DivisionByZero);
    assert_eq!(
        compiled_outcome(&p, id(&p, "run"), vec![Value::I64(0)]).unwrap(),
        pactern::pacts::Outcome::from_result(&result)
    );
}
