use std::collections::{BTreeMap, BTreeSet};

use pactern::acceptance::{Approval, HashRecord, TrustedBaseline, inventory_hash};
use pactern::editor;
use pactern::semantic::{IdentityLedger, SemanticIndex, TestDependency, digest};
use pactern::workspace::{
    ChangeRequest, ManagedWorkspace, Operation, PactTarget, SymbolTarget, Transaction, Workspace,
    WorkspaceData, source_snapshot,
};

fn files(source: &str) -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([(
        "src/main.ptn".into(),
        format!("module main; {source}").into_bytes(),
    )])
}
fn setup(source: &str, pact: Option<&str>) -> (WorkspaceData, TrustedBaseline) {
    let mut files = files(source);
    if let Some(pact) = pact {
        files.insert(
            "contracts/main.pact.ptn".into(),
            format!("module main; {pact}").into_bytes(),
        );
    }
    let data = WorkspaceData::new(files, vec![]).unwrap();
    let index = SemanticIndex::build(&source_snapshot(&data.files).unwrap(), &data.ledger).unwrap();
    let policy = TrustedBaseline::capture(&data.files, &index, &[], &[], vec![]).unwrap();
    (data, policy)
}
fn request(
    snapshot: &pactern::workspace::WorkspaceSnapshot,
    name: &str,
    body: &str,
    id: &str,
) -> ChangeRequest {
    let declaration = snapshot.index.find(name).unwrap();
    ChangeRequest {
        protocol_version: "0.1.0".into(),
        kind: "change_request".into(),
        request_id: id.into(),
        transaction: Transaction::Implementation,
        base_revision: snapshot.revision.clone(),
        expected_policy_baseline: snapshot.policy_baseline.clone(),
        operations: vec![Operation::ReplaceFunctionBody {
            target: SymbolTarget {
                symbol_id: declaration.id.clone(),
                expected_hash: declaration.declaration_hash.clone(),
            },
            body: body.into(),
        }],
    }
}

#[test]
fn ledger_rebuild_rename_move_and_body_edit_preserve_identity() {
    let first = source_snapshot(&files("fn one() -> I64 { return 1; }")).unwrap();
    let index = SemanticIndex::build(&first, &IdentityLedger::default()).unwrap();
    let id = index.find("one").unwrap().id.clone();
    let changed = source_snapshot(&files("fn two() -> I64 { return 1; }")).unwrap();
    let renamed = SemanticIndex::build(&changed, &index.ledger).unwrap();
    assert_eq!(renamed.find("two").unwrap().id, id);
    let moved_files = BTreeMap::from([(
        "src/moved.ptn".into(),
        b"module moved; fn two() -> I64 { return 1; }".to_vec(),
    )]);
    let moved =
        SemanticIndex::build(&source_snapshot(&moved_files).unwrap(), &renamed.ledger).unwrap();
    assert_eq!(moved.find("two").unwrap().id, id);
    let changed_files = BTreeMap::from([(
        "src/moved.ptn".into(),
        b"module moved; fn two() -> I64 { return 2; }".to_vec(),
    )]);
    let edited =
        SemanticIndex::build(&source_snapshot(&changed_files).unwrap(), &moved.ledger).unwrap();
    assert_eq!(edited.find("two").unwrap().id, id);
    let serialized = serde_json::to_vec(&edited.ledger).unwrap();
    let restored = serde_json::from_slice(&serialized).unwrap();
    let rebuilt =
        SemanticIndex::build(&source_snapshot(&changed_files).unwrap(), &restored).unwrap();
    assert_eq!(rebuilt.ledger, edited.ledger);
    assert_eq!(rebuilt.declarations, edited.declarations);
}

#[test]
fn ambiguous_reconciliation_is_not_guessed() {
    let first = source_snapshot(&files(
        "fn one() -> I64 { return 1; } fn two() -> I64 { return 1; }",
    ))
    .unwrap();
    let index = SemanticIndex::build(&first, &IdentityLedger::default()).unwrap();
    let next = source_snapshot(&files(
        "fn three() -> I64 { return 1; } fn four() -> I64 { return 1; }",
    ))
    .unwrap();
    assert_eq!(
        SemanticIndex::build(&next, &index.ledger).unwrap_err().code,
        "PTN_IDENTITY_AMBIGUOUS"
    );
}

#[test]
fn ids_are_never_reused_after_deletion() {
    let index = SemanticIndex::build(
        &source_snapshot(&files("fn one() -> I64 { return 1; }")).unwrap(),
        &IdentityLedger::default(),
    )
    .unwrap();
    let absent =
        SemanticIndex::build(&source_snapshot(&files("")).unwrap(), &index.ledger).unwrap();
    let restored = SemanticIndex::build(
        &source_snapshot(&files("fn one() -> I64 { return 1; }")).unwrap(),
        &absent.ledger,
    )
    .unwrap();
    assert_ne!(
        index.find("one").unwrap().id,
        restored.find("one").unwrap().id
    );
}

#[test]
fn callers_impact_and_unknown_test_dependencies_broaden() {
    let (data, policy) = setup(
        "fn helper() -> I64 { return 1; } pub fn run() -> I64 { return helper(); }",
        Some("pact positive for run { ensures result > 0; }"),
    );
    let index = data.snapshot(&policy).unwrap().index;
    assert_eq!(index.callers("helper").unwrap().result[0].name, "run");
    let impact = index.impact(&["helper".into()]).unwrap().result;
    assert_eq!(impact.interfaces.len(), 1);
    assert_eq!(impact.pacts.len(), 1);
    let tests = vec![
        TestDependency {
            test: "run_test".into(),
            declarations: BTreeSet::from([index.find("run").unwrap().id.clone()]),
            complete: true,
        },
        TestDependency {
            test: "other_test".into(),
            declarations: BTreeSet::new(),
            complete: false,
        },
    ];
    let selected = index.affected_tests(&["helper".into()], &tests).unwrap();
    assert_eq!(selected.result.len(), 2);
    assert_eq!(selected.completeness, "partial");
}

#[test]
fn transaction_replaces_body_preserves_identity_and_retries_once() {
    let (data, policy) = setup("fn run() -> I64 { return 1; }", None);
    let workspace = Workspace::new(data, policy).unwrap();
    let before = workspace.snapshot().unwrap();
    let request = request(&before, "run", "{ return 2; }", "repair-1");
    let staged = workspace.stage(&request).unwrap();
    assert_eq!(workspace.snapshot().unwrap().revision, before.revision);
    let receipt = workspace.commit(staged, None).unwrap();
    assert_ne!(receipt.candidate_revision, before.revision);
    assert_eq!(receipt.diff.len(), 2); // source and authoritative identity ledger
    assert_eq!(
        workspace.snapshot().unwrap().index.find("run").unwrap().id,
        before.index.find("run").unwrap().id
    );
    let retry = workspace.stage(&request).unwrap();
    assert_eq!(
        workspace.commit(retry, None).unwrap().candidate_revision,
        receipt.candidate_revision
    );
    let mut reused = request.clone();
    reused.operations[0] = Operation::ReplaceFunctionBody {
        target: SymbolTarget {
            symbol_id: before.index.find("run").unwrap().id.clone(),
            expected_hash: before.index.find("run").unwrap().declaration_hash.clone(),
        },
        body: "{ return 3; }".into(),
    };
    assert_eq!(
        workspace.stage(&reused).unwrap_err().code,
        "PTN_CHANGE_INVALID_REQUEST"
    );
}

#[test]
fn stale_and_invalid_requests_leave_every_byte_unchanged() {
    let (data, policy) = setup("fn run() -> I64 { return 1; }", None);
    let workspace = Workspace::new(data, policy).unwrap();
    let before = workspace.snapshot().unwrap();
    let a = workspace
        .stage(&request(&before, "run", "{ return 2; }", "a"))
        .unwrap();
    let b = workspace
        .stage(&request(&before, "run", "{ return 3; }", "b"))
        .unwrap();
    workspace.commit(a, None).unwrap();
    let committed = workspace.snapshot().unwrap();
    assert_eq!(
        workspace.commit(b, None).unwrap_err().code,
        "PTN_CHANGE_STALE_REVISION"
    );
    assert_eq!(workspace.snapshot().unwrap().files, committed.files);
    for body in [
        "{ return true; }",
        "{ return 0; } fn extra() -> I64 { return 1; }",
        "{ return 2; }",
        "{}",
    ] {
        assert!(
            workspace
                .stage(&request(&committed, "run", body, "invalid"))
                .is_err(),
            "{body}"
        );
        assert_eq!(workspace.snapshot().unwrap().files, committed.files);
    }
}

#[test]
fn multi_file_rename_updates_resolved_references_only() {
    let source = BTreeMap::from([
        ("src/helpers.ptn".into(), b"module helpers; pub fn old(x: I64) -> I64 { return x; }".to_vec()),
        ("src/main.ptn".into(), b"module main; import helpers; fn run() -> I64 { let old = 1; return helpers::old(old); } // old".to_vec()),
    ]);
    let data = WorkspaceData::new(source, vec![]).unwrap();
    let index = SemanticIndex::build(&source_snapshot(&data.files).unwrap(), &data.ledger).unwrap();
    let mut policy = TrustedBaseline::capture(&data.files, &index, &[], &[], vec![]).unwrap();
    policy.allow_public_rename = true;
    let workspace = Workspace::new(data, policy).unwrap();
    let before = workspace.snapshot().unwrap();
    let old = before.index.find("old").unwrap();
    let mut change = request(&before, "old", "{ return 2; }", "rename");
    change.transaction = Transaction::Specification;
    change.operations = vec![Operation::RenameSymbol {
        target: SymbolTarget {
            symbol_id: old.id.clone(),
            expected_hash: old.declaration_hash.clone(),
        },
        new_name: "new".into(),
    }];
    let stage = workspace.stage(&change).unwrap();
    let preview = stage.preview();
    let approval = Approval::authorize(
        preview.base_revision.clone(),
        preview.candidate_revision.clone(),
        preview.policy_baseline.clone(),
        preview.diff.clone(),
    );
    workspace.commit(stage, Some(&approval)).unwrap();
    let after = workspace.snapshot().unwrap();
    let main = String::from_utf8(after.files["src/main.ptn"].clone()).unwrap();
    assert!(main.contains("helpers::new(old)"));
    assert!(main.contains("let old = 1"));
    assert!(main.ends_with("// old"));
    assert!(
        String::from_utf8(after.files["src/helpers.ptn"].clone())
            .unwrap()
            .contains("return x")
    );
    assert_eq!(after.index.find("new").unwrap().id, old.id);
}

#[test]
fn protected_helper_and_direct_text_bypass_are_rejected() {
    let (data, policy) = setup(
        "fn helper(x: I64) -> Bool { return x > 0; } fn run() -> I64 { return 1; }",
        Some("pact positive for run { ensures helper(result); }"),
    );
    let workspace = Workspace::new(data.clone(), policy.clone()).unwrap();
    let before = workspace.snapshot().unwrap();
    assert_eq!(
        workspace
            .stage(&request(&before, "helper", "{ return true; }", "weaken"))
            .unwrap_err()
            .code,
        "PTN_CHANGE_PROTECTED_TARGET"
    );
    // Direct text bypass still encounters the exact same externally fixed gate.
    let altered = files("fn helper(x: I64) -> Bool { return true; } fn run() -> I64 { return 1; }");
    let mut changed = data.files.clone();
    changed.insert("src/main.ptn".into(), altered["src/main.ptn"].clone());
    let index = SemanticIndex::build(&source_snapshot(&changed).unwrap(), &data.ledger).unwrap();
    assert_eq!(
        policy.check(&changed, &index).unwrap_err().code,
        "PTN_CHANGE_PROTECTED_TARGET"
    );
    // The contract target implementation remains repairable.
    assert!(
        workspace
            .stage(&request(&before, "run", "{ return 2; }", "repair"))
            .is_ok()
    );
}

#[test]
fn specification_approval_binds_exact_diff_and_baseline_advancement() {
    let (data, policy) = setup(
        "fn run() -> I64 { return 1; }",
        Some("pact positive for run { ensures result > 0; }"),
    );
    let workspace = Workspace::new(data, policy).unwrap();
    let before = workspace.snapshot().unwrap();
    let pact = before.index.find("positive").unwrap();
    let mut change = request(&before, "run", "{ return 2; }", "spec");
    change.transaction = Transaction::Specification;
    change.operations = vec![Operation::ReplacePact {
        target: PactTarget {
            pact_id: pact.id.clone(),
            expected_hash: pact.declaration_hash.clone(),
        },
        declaration: "pact positive for run { ensures result >= 0; }".into(),
    }];
    let stage = workspace.stage(&change).unwrap();
    let preview = stage.preview().clone();
    assert_eq!(
        workspace.commit(stage.clone(), None).unwrap_err().code,
        "PTN_CHANGE_APPROVAL_REQUIRED"
    );
    let wrong = Approval::authorize(
        preview.base_revision.clone(),
        preview.candidate_revision.clone(),
        preview.policy_baseline.clone(),
        vec![],
    );
    assert_eq!(
        workspace
            .commit(stage.clone(), Some(&wrong))
            .unwrap_err()
            .code,
        "PTN_CHANGE_APPROVAL_REQUIRED"
    );
    let approval = Approval::authorize(
        preview.base_revision,
        preview.candidate_revision,
        preview.policy_baseline,
        preview.diff,
    );
    assert!(
        workspace
            .commit(stage, Some(&approval))
            .unwrap()
            .baseline_advancement_pending
    );
    let after = workspace.snapshot().unwrap();
    assert_eq!(
        workspace
            .stage(&request(&after, "run", "{ return 3; }", "pending"))
            .unwrap_err()
            .code,
        "PTN_CHANGE_PROTECTED_TARGET"
    );
}

#[test]
fn overlapping_rename_and_body_footprints_are_rejected() {
    let (data, policy) = setup(
        "fn helper() -> I64 { return 1; } fn run() -> I64 { return helper(); }",
        None,
    );
    let workspace = Workspace::new(data, policy).unwrap();
    let before = workspace.snapshot().unwrap();
    let mut change = request(&before, "run", "{ return 2; }", "overlap");
    let helper = before.index.find("helper").unwrap();
    change.operations.push(Operation::RenameSymbol {
        target: SymbolTarget {
            symbol_id: helper.id.clone(),
            expected_hash: helper.declaration_hash.clone(),
        },
        new_name: "other".into(),
    });
    assert_eq!(
        workspace.stage(&change).unwrap_err().code,
        "PTN_CHANGE_INVALID_REQUEST"
    );
    assert_eq!(workspace.snapshot().unwrap().files, before.files);
}

#[test]
fn required_checks_cannot_be_claimed_by_the_request() {
    let (data, mut policy) = setup("fn run() -> I64 { return 1; }", None);
    policy.require_check("regression-suite");
    let workspace = Workspace::new(data, policy).unwrap();
    let before = workspace.snapshot().unwrap();
    let change = request(&before, "run", "{ return 2; }", "checked");
    assert!(workspace.stage(&change).is_err());
    assert!(
        workspace
            .stage_with(&change, |_| Ok(BTreeSet::from(["regression-suite".into()])))
            .is_ok()
    );
}

#[test]
fn strict_request_decode_rejects_duplicate_and_unknown_fields() {
    let (data, policy) = setup("fn run() -> I64 { return 1; }", None);
    let before = data.snapshot(&policy).unwrap();
    let change = request(&before, "run", "{ return 2; }", "decode");
    let json = serde_json::to_string(&change).unwrap();
    assert!(ChangeRequest::decode(json.as_bytes()).is_ok());
    let duplicate = json.replacen("\"kind\":", "\"kind\":\"change_request\",\"kind\":", 1);
    assert!(ChangeRequest::decode(duplicate.as_bytes()).is_err());
    let nested = json.replacen(
        "\"symbol_id\":",
        "\"symbol_id\":\"duplicate\",\"symbol_id\":",
        1,
    );
    assert!(ChangeRequest::decode(nested.as_bytes()).is_err());
    let unknown = json.replacen('{', "{\"approved\":true,", 1);
    assert!(ChangeRequest::decode(unknown.as_bytes()).is_err());
    assert!(ChangeRequest::decode(format!("{json} null").as_bytes()).is_err());
}

#[test]
fn inventory_hash_binds_all_inputs_and_rejects_duplicate_records() {
    let a = HashRecord {
        category: "file".into(),
        key: "a".into(),
        digest: digest("first"),
    };
    let b = HashRecord {
        category: "file".into(),
        key: "b".into(),
        digest: digest("second"),
    };
    assert_eq!(
        inventory_hash("workspace", &[a.clone(), b.clone()]).unwrap(),
        inventory_hash("workspace", &[b, a.clone()]).unwrap()
    );
    assert!(inventory_hash("workspace", &[a.clone(), a]).is_err());
    let (mut data, policy) = setup("fn run() -> I64 { return 1; }", None);
    let before = data.snapshot(&policy).unwrap().revision;
    data.files
        .insert("fixtures/untracked.json".into(), b"{}".to_vec());
    assert_ne!(data.snapshot(&policy).unwrap().revision, before);
}

#[test]
fn editor_utf16_and_definition_are_revision_bound() {
    assert_eq!(
        editor::position("a😀é\r\nx", 5).unwrap(),
        editor::Position {
            line: 0,
            character: 3
        }
    );
    assert_eq!(
        editor::position("a😀é\r\nx", 9).unwrap(),
        editor::Position {
            line: 1,
            character: 0
        }
    );
    assert!(editor::position("é", 1).is_err());
    let (data, policy) = setup(
        "fn helper() -> I64 { return 1; } fn run() -> I64 { return helper(); }",
        None,
    );
    let snapshot = data.snapshot(&policy).unwrap();
    let reference = snapshot
        .index
        .references
        .iter()
        .find(|r| r.owner == snapshot.index.find("run").unwrap().id)
        .unwrap();
    let target = editor::definition(
        &snapshot.index,
        &snapshot.revision,
        &reference.range.path,
        reference.range.start_byte,
    )
    .unwrap()
    .result
    .unwrap();
    assert_eq!(target, snapshot.index.find("helper").unwrap().name_range);
    assert!(
        editor::definition(
            &snapshot.index,
            "old",
            &reference.range.path,
            reference.range.start_byte
        )
        .is_err()
    );
    let invalid = source_snapshot(&files("fn bad() -> I64 { return true; }")).unwrap();
    let diagnostics = editor::diagnostics(&invalid).unwrap();
    assert!(!diagnostics.result.is_empty());
    assert!(diagnostics.result[0].range.is_some());
}

#[test]
fn managed_store_publishes_complete_snapshots_and_reopens_after_commit() {
    let (data, policy) = setup("fn run() -> I64 { return 1; }", None);
    let directory = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("pactern-managed-{}", &data.ledger.namespace[7..39]));
    let store = ManagedWorkspace::create(&directory, &data).unwrap();
    let before = store.snapshot(&policy).unwrap();
    let change = request(&before, "run", "{ return 2; }", "disk");
    let staged = store.stage(&policy, &change).unwrap();
    let receipt = store.commit(&policy, staged, None).unwrap();
    let reopened = ManagedWorkspace::open(&directory).unwrap();
    assert_eq!(
        reopened.snapshot(&policy).unwrap().revision,
        receipt.candidate_revision
    );
    let retried = reopened.stage(&policy, &change).unwrap();
    assert_eq!(
        reopened
            .commit(&policy, retried, None)
            .unwrap()
            .candidate_revision,
        receipt.candidate_revision
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn concurrent_commits_publish_exactly_one_candidate() {
    let (data, policy) = setup("fn run() -> I64 { return 1; }", None);
    let workspace = Workspace::new(data, policy).unwrap();
    let before = workspace.snapshot().unwrap();
    let mut threads = vec![];
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    for i in 0..8 {
        let staged = workspace
            .stage(&request(
                &before,
                "run",
                &format!("{{ return {}; }}", i + 2),
                &format!("candidate-{i}"),
            ))
            .unwrap();
        let workspace = workspace.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            workspace.commit(staged, None)
        }));
    }
    let successes = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .filter(Result::is_ok)
        .count();
    assert_eq!(successes, 1);
}

#[test]
fn managed_inputs_reject_case_aliases_and_reserved_ledger_paths() {
    let aliases = BTreeMap::from([
        ("src/a.ptn".into(), b"module a;".to_vec()),
        ("src/A.ptn".into(), b"module A;".to_vec()),
    ]);
    assert!(WorkspaceData::new(aliases, vec![]).is_err());
    let (mut data, policy) = setup("fn run()->I64{return 1;}", None);
    data.files
        .insert(".PACTERN/identities.json".into(), b"{}".to_vec());
    assert!(data.snapshot(&policy).is_err());
}

#[test]
fn braces_in_pact_header_comments_cannot_rebind_a_target() {
    let (data, policy) = setup(
        "fn run()->I64{return 1;} fn other()->I64{return 2;}",
        Some("pact positive /* { */ for run {ensures result > 0;}"),
    );
    let workspace = Workspace::new(data, policy).unwrap();
    let before = workspace.snapshot().unwrap();
    // A comment brace must not accidentally protect the target implementation.
    assert!(
        workspace
            .stage(&request(&before, "run", "{return 3;}", "repair"))
            .is_ok()
    );
    let pact = before.index.find("positive").unwrap();
    let mut change = request(&before, "run", "{return 3;}", "rebind");
    change.transaction = Transaction::Specification;
    change.operations = vec![Operation::ReplacePact {
        target: PactTarget {
            pact_id: pact.id.clone(),
            expected_hash: pact.declaration_hash.clone(),
        },
        declaration: "pact positive /* { */ for other {ensures result > 0;}".into(),
    }];
    assert_eq!(
        workspace.stage(&change).unwrap_err().code,
        "PTN_CHANGE_INVALID_REQUEST"
    );
}
