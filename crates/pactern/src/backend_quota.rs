//! Opt-in bridge for the sole versioned quota capability ABI.
use super::{BackendError, Execution, HostLimits, Input, Result};
use crate::{
    core::Program,
    host::{Handle, HostError, Invocation, Trace},
    interpreter::{Limits, Value},
    resolution::SymbolId,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub enum QuotaArgument {
    Value(Value),
    HandleSlot(u32),
}
/// Execution failure and trace finalization are separate so a trap can be replayed.
#[derive(Debug)]
pub struct QuotaExecution {
    pub outcome: Result<Execution>,
    pub trace: std::result::Result<Trace, HostError>,
}
pub fn emit_c_quota(
    program: &Program,
    entry: SymbolId,
    args: &[QuotaArgument],
    limits: Limits,
) -> Result<String> {
    let args: Vec<_> = args
        .iter()
        .map(|a| match a {
            QuotaArgument::Value(v) => Input::Value(v),
            QuotaArgument::HandleSlot(s) => Input::HandleSlot(*s),
        })
        .collect();
    super::emit(program, entry, &args, limits, true, super::Bridge::Quota)
}
/// Consumes the invocation and the exact set of handles granted to this guest.
/// Every exit revokes authority. Completed earlier commits remain committed;
/// pending writes roll back. Replay finalization rejects an unread trace tail.
pub fn run_wasm_quota(
    bytes: &[u8],
    limits: HostLimits,
    mut invocation: Invocation,
    handles: Vec<Handle>,
) -> QuotaExecution {
    let outcome = run_inner(bytes, limits, &mut invocation, handles);
    invocation.revoke();
    let trace = invocation.finish();
    QuotaExecution { outcome, trace }
}
struct State<'a> {
    limits: wasmi::StoreLimits,
    invocation: &'a mut Invocation,
    roots: Vec<Option<Handle>>,
    tokens: BTreeMap<u32, Handle>,
    next_token: u32,
}
fn host_error(error: HostError) -> wasmi::Error {
    wasmi::Error::new(error.to_string())
}
fn denied() -> wasmi::Error {
    host_error(HostError::Denied)
}
fn run_inner(
    bytes: &[u8],
    limits: HostLimits,
    invocation: &mut Invocation,
    handles: Vec<Handle>,
) -> Result<Execution> {
    use wasmi::{Caller, Config, Engine, Linker, Module, Store, StoreLimitsBuilder};
    invocation
        .checkpoint()
        .map_err(|e| BackendError(e.to_string()))?;
    if bytes.len() > limits.module_bytes {
        return Err(BackendError("module byte limit exceeded".into()));
    }
    if handles.len() > 1024 {
        return Err(BackendError("quota root handle limit exceeded".into()));
    }
    let mut config = Config::default();
    config.consume_fuel(true);
    config.compilation_mode(wasmi::CompilationMode::Eager);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, bytes).map_err(|e| BackendError(e.to_string()))?;
    invocation
        .checkpoint()
        .map_err(|e| BackendError(e.to_string()))?;
    for import in module.imports() {
        if import.module() != "pactern.quota.v1"
            || !["root", "read", "reserve", "commit", "rollback", "release"]
                .contains(&import.name())
        {
            return Err(BackendError(format!(
                "quota ABI import denied: {}::{}",
                import.module(),
                import.name()
            )));
        }
    }
    let resources = StoreLimitsBuilder::new()
        .memory_size(limits.memory_bytes)
        .memories(1)
        .tables(1)
        .table_elements(1024)
        .instances(1)
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(
        &engine,
        State {
            limits: resources,
            invocation,
            roots: handles.into_iter().map(Some).collect(),
            tokens: BTreeMap::new(),
            next_token: 1,
        },
    );
    store.limiter(|s| &mut s.limits);
    let initial_fuel = limits.fuel.min(10_000);
    store
        .set_fuel(initial_fuel)
        .map_err(|e| BackendError(e.to_string()))?;
    let mut linker = Linker::new(&engine);
    linker
        .func_wrap(
            "pactern.quota.v1",
            "root",
            |mut caller: Caller<'_, State<'_>>,
             slot: u32|
             -> std::result::Result<u32, wasmi::Error> {
                let state = caller.data_mut();
                state.invocation.checkpoint().map_err(host_error)?;
                let handle = state
                    .roots
                    .get_mut(slot as usize)
                    .and_then(Option::take)
                    .ok_or_else(denied)?;
                let token = state.next_token;
                state.next_token = token.checked_add(1).ok_or_else(denied)?;
                state.tokens.insert(token, handle);
                Ok(token)
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    linker
        .func_wrap(
            "pactern.quota.v1",
            "read",
            |mut caller: Caller<'_, State<'_>>,
             token: u32|
             -> std::result::Result<i64, wasmi::Error> {
                let state = caller.data_mut();
                let handle = state.tokens.get(&token).ok_or_else(denied)?;
                state.invocation.read(handle).map_err(host_error)
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    linker
        .func_wrap(
            "pactern.quota.v1",
            "reserve",
            |mut caller: Caller<'_, State<'_>>,
             token: u32,
             amount: i64|
             -> std::result::Result<u32, wasmi::Error> {
                let state = caller.data_mut();
                let handle = state.tokens.get(&token).ok_or_else(denied)?;
                state
                    .invocation
                    .reserve(handle, amount)
                    .map(u32::from)
                    .map_err(host_error)
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    linker
        .func_wrap(
            "pactern.quota.v1",
            "commit",
            |mut caller: Caller<'_, State<'_>>,
             token: u32|
             -> std::result::Result<(), wasmi::Error> {
                let state = caller.data_mut();
                let handle = state.tokens.remove(&token).ok_or_else(denied)?;
                state.invocation.commit(handle).map_err(host_error)
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    linker
        .func_wrap(
            "pactern.quota.v1",
            "rollback",
            |mut caller: Caller<'_, State<'_>>,
             token: u32|
             -> std::result::Result<(), wasmi::Error> {
                let state = caller.data_mut();
                let handle = state.tokens.remove(&token).ok_or_else(denied)?;
                state.invocation.rollback(handle).map_err(host_error)
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    linker
        .func_wrap(
            "pactern.quota.v1",
            "release",
            |mut caller: Caller<'_, State<'_>>,
             token: u32|
             -> std::result::Result<(), wasmi::Error> {
                let state = caller.data_mut();
                let handle = state.tokens.remove(&token).ok_or_else(denied)?;
                state.invocation.release(handle).map_err(host_error)
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    // Linker type checking rejects non-function imports and ABI signature spoofing.
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| BackendError(format!("quota ABI instantiation failed: {e}")))?;
    super::execute_instance(
        store,
        instance,
        limits,
        limits.fuel - initial_fuel,
        |state| {
            state
                .invocation
                .checkpoint()
                .map_err(|e| BackendError(e.to_string()))
        },
    )
}
