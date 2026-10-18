//! Independent adversarial modules exercise host boundaries without the emitter.
use pactern::{
    backend::{self, HostLimits},
    host::*,
};
use std::{collections::BTreeSet, sync::Arc};

fn grant() -> Grant {
    Grant {
        tenant: "a".into(),
        operations: BTreeSet::from([Operation::Read, Operation::Write]),
    }
}
fn invocation(store: &Arc<QuotaStore>) -> Invocation {
    Invocation::record(
        store.clone(),
        vec![grant()],
        Budget::default(),
        "revision",
        "policy",
    )
}
fn leb(mut value: usize) -> Vec<u8> {
    let mut bytes = vec![];
    loop {
        let byte = (value & 127) as u8;
        value >>= 7;
        bytes.push(byte | if value != 0 { 128 } else { 0 });
        if value == 0 {
            return bytes;
        }
    }
}
fn section(module: &mut Vec<u8>, id: u8, content: &[u8]) {
    module.push(id);
    module.extend(leb(content.len()));
    module.extend(content);
}
fn string(out: &mut Vec<u8>, value: &str) {
    out.extend(leb(value.len()));
    out.extend(value.as_bytes());
}

#[test]
fn start_functions_obey_fuel_and_memory_limits_and_revoke_pending_transactions() {
    let mut looping = b"\0asm\x01\0\0\0".to_vec();
    section(&mut looping, 1, &[1, 0x60, 0, 0]); // one () -> () type
    section(&mut looping, 3, &[1, 0]); // function type 0
    section(&mut looping, 8, &[0]); // start function 0
    section(
        &mut looping,
        10,
        &[1, 7, 0, 0x03, 0x40, 0x0c, 0, 0x0b, 0x0b],
    );
    let mut excessive_memory = b"\0asm\x01\0\0\0".to_vec();
    let mut memory = vec![1, 0];
    memory.extend(leb(257));
    section(&mut excessive_memory, 5, &memory);
    for (module, expected) in [(looping, "fuel"), (excessive_memory, "memory")] {
        let store = Arc::new(QuotaStore::new([("a".into(), 100)]).unwrap());
        let mut invocation = invocation(&store);
        let handle = invocation.begin("a").unwrap();
        invocation.reserve(&handle, 30).unwrap();
        let result =
            backend::run_wasm_quota(&module, HostLimits::default(), invocation, vec![handle]);
        let error = result.outcome.unwrap_err().to_string().to_lowercase();
        assert!(error.contains(expected), "{error}");
        assert_eq!(result.trace.unwrap().entries.len(), 2);
        assert_eq!(store.balance("a").unwrap(), 100);
        assert_eq!(store.live_writes(), 0);
    }
}

#[test]
fn allowed_import_name_cannot_smuggle_an_imported_memory() {
    let mut module = b"\0asm\x01\0\0\0".to_vec();
    let mut import = vec![1];
    string(&mut import, "pactern.quota.v1");
    string(&mut import, "root");
    import.extend([2, 0, 1]);
    section(&mut module, 2, &import);
    let store = Arc::new(QuotaStore::new([("a".into(), 100)]).unwrap());
    let result =
        backend::run_wasm_quota(&module, HostLimits::default(), invocation(&store), vec![]);
    assert!(
        result
            .outcome
            .unwrap_err()
            .to_string()
            .contains("instantiation failed")
    );
    assert_eq!(store.live_writes(), 0);
}

#[test]
fn foreign_invocation_tokens_and_redacted_bridge_replay_cannot_write() {
    let c = r#"
    __attribute__((import_module("pactern.quota.v1"),import_name("root"))) unsigned root(unsigned);
    __attribute__((import_module("pactern.quota.v1"),import_name("read"))) long long rd(unsigned);
    static unsigned char output[1]={0};
    unsigned pt_run(void){rd(root(0));return 0;}
    unsigned pt_abi_version(void){return 1;}
    unsigned pt_output_ptr(void){return (unsigned)output;}
    unsigned pt_output_len(void){return 1;}
    unsigned pt_trap_cause(void){return 0;}
    unsigned pt_collections(void){return 0;}
    "#;
    let module = backend::compile_wasm(c).unwrap();
    let store = Arc::new(QuotaStore::new([("a".into(), 100)]).unwrap());
    let mut foreign = invocation(&store);
    let handle = foreign.begin("a").unwrap();
    let result = backend::run_wasm_quota(
        &module,
        HostLimits::default(),
        invocation(&store),
        vec![handle],
    );
    assert!(result.outcome.unwrap_err().to_string().contains("Revoked"));
    result.trace.unwrap();
    let mut live = invocation(&store);
    let handle = live.begin("a").unwrap();
    let recorded = backend::run_wasm_quota(&module, HostLimits::default(), live, vec![handle]);
    recorded.outcome.unwrap();
    let mut trace = recorded.trace.unwrap();
    trace.entries[1].response = None;
    let mut replay = Invocation::replay(
        store.clone(),
        vec![grant()],
        Budget::default(),
        "revision",
        "policy",
        trace,
    )
    .unwrap();
    let handle = replay.begin("a").unwrap();
    let result = backend::run_wasm_quota(&module, HostLimits::default(), replay, vec![handle]);
    assert!(
        result
            .outcome
            .unwrap_err()
            .to_string()
            .contains("MissingRecording")
    );
    assert!(matches!(result.trace, Err(HostError::IncompleteReplay)));
    assert_eq!(store.live_writes(), 0);
}
