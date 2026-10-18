use pactern::{
    backend::{self, Execution, HostLimits, QuotaArgument},
    core,
    host::{Budget, Grant, HostError, Invocation, Operation, QuotaStore},
    interpreter::{Limits, TrapKind, Value},
    resolution::{ParsedFile, SymbolKind},
    syntax::{FileKind, SourceFile, parse},
};
use std::{collections::BTreeSet, sync::Arc};
fn grant() -> Grant {
    Grant {
        tenant: "a".into(),
        operations: BTreeSet::from([Operation::Read, Operation::Write]),
    }
}
fn store() -> Arc<QuotaStore> {
    Arc::new(QuotaStore::new([("a".into(), 100), ("b".into(), 9)]).unwrap())
}
fn invocation(s: &Arc<QuotaStore>, budget: Budget) -> Invocation {
    Invocation::record(s.clone(), vec![grant()], budget, "revision", "policy")
}
fn wasm(source: &str, name: &str, args: &[QuotaArgument], fuel: u64) -> Vec<u8> {
    let source = SourceFile::new("src/transaction.ptn", source);
    let p = core::lower(&[ParsedFile::new(
        source.name(),
        parse(&source, FileKind::Implementation).unwrap(),
    )])
    .unwrap();
    let f = p
        .resolution()
        .symbols
        .iter()
        .find(|s| s.kind == SymbolKind::Function && s.name == name)
        .unwrap()
        .id;
    let c = backend::emit_c_quota(
        &p,
        f,
        args,
        Limits {
            fuel,
            call_depth: 64,
        },
    )
    .unwrap();
    backend::compile_wasm(&c).unwrap()
}
fn reservation(amount: i64) -> Vec<u8> {
    wasm(
        include_str!("../../../examples/quota/src/transaction.ptn"),
        "reserve",
        &[
            QuotaArgument::HandleSlot(0),
            QuotaArgument::Value(Value::I64(amount)),
        ],
        100_000,
    )
}
#[test]
fn compiled_pactern_borrows_reserves_and_consumes_host_transaction() {
    let bytes = reservation(30);
    assert!(
        backend::run_wasm(&bytes, HostLimits::default())
            .unwrap_err()
            .to_string()
            .contains("ambient import denied")
    );
    let s = store();
    let mut i = invocation(&s, Budget::default());
    let handle = i.begin("a").unwrap();
    let cancelled = i.cancellation();
    let result = backend::run_wasm_quota(&bytes, HostLimits::default(), i, vec![handle]);
    assert_eq!(result.outcome.unwrap(), Execution::Value(Value::Bool(true)));
    let trace = result.trace.unwrap();
    assert_eq!(trace.entries.len(), 4);
    assert_eq!(s.balance("a").unwrap(), 70);
    assert_eq!(s.balance("b").unwrap(), 9);
    assert_eq!(s.live_writes(), 1);
    cancelled.cancel();
    let mut replay = Invocation::replay(
        s.clone(),
        vec![grant()],
        Budget::default(),
        "revision",
        "policy",
        trace,
    )
    .unwrap();
    let handle = replay.begin("a").unwrap();
    let replayed = backend::run_wasm_quota(&bytes, HostLimits::default(), replay, vec![handle]);
    assert_eq!(
        replayed.outcome.unwrap(),
        Execution::Value(Value::Bool(true))
    );
    replayed.trace.unwrap();
    assert_eq!(s.live_writes(), 1);
}
#[test]
fn traps_and_failed_reservations_leave_no_pending_writes() {
    let source = "module transaction;fn run(t:host::quota::Txn)->I64 effects{db.write}{let accepted=host::quota::reserve(borrow t,30);return [1][2];}";
    let bytes = wasm(source, "run", &[QuotaArgument::HandleSlot(0)], 100_000);
    let s = store();
    let mut i = invocation(&s, Budget::default());
    let h = i.begin("a").unwrap();
    let result = backend::run_wasm_quota(&bytes, HostLimits::default(), i, vec![h]);
    assert_eq!(
        result.outcome.unwrap(),
        Execution::Trap {
            kind: TrapKind::IndexOutOfBounds,
            cause: None
        }
    );
    assert_eq!(s.balance("a").unwrap(), 100);
    assert_eq!(s.live_writes(), 0);
    assert_eq!(result.trace.unwrap().entries.len(), 2);
    let mut i = invocation(&s, Budget::default());
    let h = i.begin("a").unwrap();
    let result = backend::run_wasm_quota(&reservation(101), HostLimits::default(), i, vec![h]);
    assert_eq!(
        result.outcome.unwrap(),
        Execution::Value(Value::Bool(false))
    );
    result.trace.unwrap();
    assert_eq!(s.live_writes(), 0);
}
#[test]
fn operation_budget_cancellation_and_deadline_interrupt_compiled_execution() {
    let bytes = reservation(1);
    let s = store();
    let mut i = invocation(
        &s,
        Budget {
            operations: 1,
            ..Budget::default()
        },
    );
    let h = i.begin("a").unwrap();
    let result = backend::run_wasm_quota(&bytes, HostLimits::default(), i, vec![h]);
    assert!(
        result
            .outcome
            .unwrap_err()
            .to_string()
            .contains("BudgetExceeded")
    );
    result.trace.unwrap();
    assert_eq!(s.live_writes(), 0);
    let source = "module transaction;fn run(t:host::quota::Txn)->Unit effects{db.write}{let accepted=host::quota::reserve(borrow t,1);while(true){} }";
    let bytes = wasm(source, "run", &[QuotaArgument::HandleSlot(0)], u64::MAX);
    let mut i = invocation(&s, Budget::default());
    let h = i.begin("a").unwrap();
    let cancellation = i.cancellation();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(10));
        cancellation.cancel();
    });
    let result = backend::run_wasm_quota(
        &bytes,
        HostLimits {
            fuel: u64::MAX,
            ..HostLimits::default()
        },
        i,
        vec![h],
    );
    thread.join().unwrap();
    assert!(result.outcome.unwrap_err().to_string().contains("Revoked"));
    result.trace.unwrap();
    assert_eq!(s.live_writes(), 0);
    let mut i = invocation(
        &s,
        Budget {
            timeout_ms: 20,
            ..Budget::default()
        },
    );
    let h = i.begin("a").unwrap();
    let result = backend::run_wasm_quota(
        &bytes,
        HostLimits {
            fuel: u64::MAX,
            ..HostLimits::default()
        },
        i,
        vec![h],
    );
    assert!(result.outcome.unwrap_err().to_string().contains("Timeout"));
    result.trace.unwrap();
    assert_eq!(s.live_writes(), 0);
}
fn raw(imports: &str, body: &str) -> Vec<u8> {
    let c = format!(
        "{imports}\nunsigned pt_run(void){{{body}}}unsigned pt_abi_version(void){{return 1;}}unsigned pt_output_ptr(void){{return 0;}}unsigned pt_output_len(void){{return 0;}}unsigned pt_trap_cause(void){{return 0;}}unsigned pt_collections(void){{return 0;}}"
    );
    backend::compile_wasm(&c).unwrap()
}
#[test]
fn forged_tokens_repeated_roots_and_import_signature_spoofs_are_denied() {
    let s = store();
    let cases = [
        raw(
            "__attribute__((import_module(\"pactern.quota.v1\"),import_name(\"read\"))) long long rd(unsigned);",
            "rd(999);return 0;",
        ),
        raw(
            "__attribute__((import_module(\"pactern.quota.v1\"),import_name(\"root\"))) unsigned root(unsigned);",
            "root(0);root(0);return 0;",
        ),
        raw(
            "__attribute__((import_module(\"pactern.quota.v1\"),import_name(\"read\"))) int rd(void);",
            "rd();return 0;",
        ),
        raw(
            "__attribute__((import_module(\"pactern.quota.v2\"),import_name(\"read\"))) long long rd(unsigned);",
            "rd(0);return 0;",
        ),
        raw(
            "__attribute__((import_module(\"pactern.quota.v1\"),import_name(\"root\"))) unsigned root(unsigned);__attribute__((import_module(\"pactern.quota.v1\"),import_name(\"rollback\"))) void rollback(unsigned);__attribute__((import_module(\"pactern.quota.v1\"),import_name(\"read\"))) long long rd(unsigned);",
            "unsigned token=root(0);rollback(token);rd(token);return 0;",
        ),
    ];
    for bytes in cases {
        let mut i = invocation(&s, Budget::default());
        let h = i.begin("a").unwrap();
        let result = backend::run_wasm_quota(&bytes, HostLimits::default(), i, vec![h]);
        assert!(result.outcome.is_err());
        result.trace.unwrap();
        assert_eq!(s.live_writes(), 0);
    }
}
#[test]
fn replay_rejects_changed_requests_without_external_writes() {
    let s = store();
    let mut i = invocation(&s, Budget::default());
    let h = i.begin("a").unwrap();
    let result = backend::run_wasm_quota(&reservation(2), HostLimits::default(), i, vec![h]);
    result.outcome.unwrap();
    let trace = result.trace.unwrap();
    let mut replay = Invocation::replay(
        s.clone(),
        vec![grant()],
        Budget::default(),
        "revision",
        "policy",
        trace,
    )
    .unwrap();
    let h = replay.begin("a").unwrap();
    let result = backend::run_wasm_quota(&reservation(3), HostLimits::default(), replay, vec![h]);
    assert!(
        result
            .outcome
            .unwrap_err()
            .to_string()
            .contains("ReplayMismatch")
    );
    assert!(matches!(result.trace, Err(HostError::IncompleteReplay)));
    assert_eq!(s.live_writes(), 1);
}

#[test]
fn borrowed_helpers_retain_owners_and_implicit_owning_cleanup_is_unmetered() {
    let borrowed = "module transaction;fn peek(t:borrow host::quota::Txn)->I64 effects{db.read}{return host::quota::read(borrow t);}fn run(t:host::quota::Txn)->I64 effects{db.read}{let a=peek(borrow t);return peek(borrow t);}";
    let bytes = wasm(borrowed, "run", &[QuotaArgument::HandleSlot(0)], 100_000);
    let s = store();
    let mut i = invocation(
        &s,
        Budget {
            operations: 3,
            ..Budget::default()
        },
    );
    let h = i.begin("a").unwrap();
    let result = backend::run_wasm_quota(&bytes, HostLimits::default(), i, vec![h]);
    assert_eq!(result.outcome.unwrap(), Execution::Value(Value::I64(100)));
    assert_eq!(result.trace.unwrap().entries.len(), 3);
    assert_eq!(s.live_writes(), 0);
    let owning = "module transaction;fn discard(t:host::quota::Txn)->Unit{}fn run(t:host::quota::Txn)->Unit{discard(t);}";
    let source = SourceFile::new("src/transaction.ptn", owning);
    let p = core::lower(&[ParsedFile::new(
        source.name(),
        parse(&source, FileKind::Implementation).unwrap(),
    )])
    .unwrap();
    let f = p
        .resolution()
        .symbols
        .iter()
        .find(|s| s.name == "run" && s.kind == SymbolKind::Function)
        .unwrap()
        .id;
    let c =
        backend::emit_c_quota(&p, f, &[QuotaArgument::HandleSlot(0)], Limits::default()).unwrap();
    // A malicious ABI call after the owning helper returned cannot revive its token.
    let c = c.replace(
        "if(!trap_code)encode",
        "pt_quota_read(1);if(!trap_code)encode",
    );
    let bytes = backend::compile_wasm(&c).unwrap();
    let mut i = invocation(
        &s,
        Budget {
            operations: 1,
            ..Budget::default()
        },
    );
    let h = i.begin("a").unwrap();
    let result = backend::run_wasm_quota(&bytes, HostLimits::default(), i, vec![h]);
    assert!(result.outcome.unwrap_err().to_string().contains("Denied"));
    assert_eq!(result.trace.unwrap().entries.len(), 1);
}

#[test]
fn moved_block_local_is_released_before_the_next_statement() {
    let text = "module transaction;fn run(t:host::quota::Txn,u:host::quota::Txn)->I64 effects{db.read}{{let moved=t;}return host::quota::read(borrow u);}";
    let source = SourceFile::new("src/transaction.ptn", text);
    let p = core::lower(&[ParsedFile::new(
        source.name(),
        parse(&source, FileKind::Implementation).unwrap(),
    )])
    .unwrap();
    let f = p
        .resolution()
        .symbols
        .iter()
        .find(|s| s.name == "run" && s.kind == SymbolKind::Function)
        .unwrap()
        .id;
    let c = backend::emit_c_quota(
        &p,
        f,
        &[QuotaArgument::HandleSlot(0), QuotaArgument::HandleSlot(1)],
        Limits::default(),
    )
    .unwrap();
    // Replace the next source statement's read with a stale-token attack. If the
    // inner scope retained its moved owner this would successfully read token 1.
    let c = c.replace("pt_quota_read((U32)", "pt_quota_read(1+(0*(U32)");
    let c = c.replace("].n));", "].n)));");
    // Keep mutation narrowly targeted: the only scalar host read in this program.
    let bytes = backend::compile_wasm(&c).unwrap();
    let s = store();
    let mut i = invocation(&s, Budget::default());
    let a = i.begin("a").unwrap();
    let b = i.begin("a").unwrap();
    let result = backend::run_wasm_quota(&bytes, HostLimits::default(), i, vec![a, b]);
    assert!(result.outcome.unwrap_err().to_string().contains("Denied"));
    assert_eq!(result.trace.unwrap().entries.len(), 2);
}
