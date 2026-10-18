//! Explicitly granted terminal I/O and UTF-8 process arguments for Wasm guests.
use super::{BackendError, Bridge, Execution, HostLimits, Input, Result};
use crate::{core::Program, interpreter::Limits, resolution::SymbolId};
use std::io::{BufRead, Write};

/// Streams are supplied by the embedding application, never discovered by guests.
/// Arguments exclude the executable name. Reads and writes are cumulatively bounded.
pub struct TerminalIo<'a> {
    pub input: &'a mut dyn BufRead,
    pub output: &'a mut dyn Write,
    pub error: &'a mut dyn Write,
    pub arguments: &'a [String],
    pub byte_limit: usize,
}

/// The entry receives a single owning Terminal handle; guards are optional.
pub fn emit_c_terminal(
    program: &Program,
    entry: SymbolId,
    limits: Limits,
    guarded: bool,
) -> Result<String> {
    let valid = program.declarations().iter().any(|d| d.symbol == entry && matches!(&d.kind,
        crate::core::DeclarationKind::Function { parameters, .. }
            if parameters.len() == 1 && !program.borrowed_parameters().contains(&parameters[0])
                && matches!(program.symbol_types().get(&parameters[0]), Some(crate::typing::ValueType::Named(id))
                    if program.resolution().symbols[id.0].name == "host::terminal::Terminal")));
    if !valid {
        return Err(BackendError(
            "terminal entry must take exactly one owning host::terminal::Terminal parameter".into(),
        ));
    }
    super::emit(
        program,
        entry,
        &[Input::HandleSlot(0)],
        limits,
        guarded,
        Bridge::Terminal,
    )
}

struct State<'a> {
    limits: wasmi::StoreLimits,
    io: TerminalIo<'a>,
    remaining: usize,
    claimed: bool,
    live: bool,
}
impl State<'_> {
    fn check(&self, token: u32) -> std::result::Result<(), wasmi::Error> {
        if token != 1 || !self.live {
            return Err(wasmi::Error::new("terminal handle denied"));
        }
        Ok(())
    }
    fn charge(&mut self, bytes: usize) -> std::result::Result<(), wasmi::Error> {
        self.remaining = self
            .remaining
            .checked_sub(bytes)
            .ok_or_else(|| wasmi::Error::new("terminal byte limit exceeded"))?;
        Ok(())
    }
}
fn memory(
    caller: &wasmi::Caller<'_, State<'_>>,
) -> std::result::Result<wasmi::Memory, wasmi::Error> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmi::Error::new("terminal guest has no exported memory"))
}
fn io_error(error: impl std::fmt::Display) -> wasmi::Error {
    wasmi::Error::new(format!("terminal I/O failed: {error}"))
}

/// Only the terminal ABI is linked. Pure and quota hosts still deny these imports.
/// Stream errors trap; EOF is Option::None, and empty lines are Option::Some("").
pub fn run_wasm_terminal(
    bytes: &[u8],
    limits: HostLimits,
    io: TerminalIo<'_>,
) -> Result<Execution> {
    use wasmi::{Caller, Config, Engine, Linker, Module, Store, StoreLimitsBuilder};
    if bytes.len() > limits.module_bytes {
        return Err(BackendError("module byte limit exceeded".into()));
    }
    let argument_bytes = io
        .arguments
        .iter()
        .try_fold(0usize, |total, arg| total.checked_add(arg.len()));
    if io.arguments.len() > 1024
        || argument_bytes.is_none_or(|n| n > io.byte_limit || n > u32::MAX as usize)
    {
        return Err(BackendError("terminal argument limit exceeded".into()));
    }
    let mut config = Config::default();
    config.consume_fuel(true);
    config.compilation_mode(wasmi::CompilationMode::Eager);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, bytes).map_err(|e| BackendError(e.to_string()))?;
    for import in module.imports() {
        if import.module() != "pactern.terminal.v1"
            || ![
                "root",
                "release",
                "argc",
                "arg_len",
                "arg_copy",
                "read_line",
                "write",
            ]
            .contains(&import.name())
        {
            return Err(BackendError(format!(
                "terminal ABI import denied: {}::{}",
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
    let remaining = io.byte_limit;
    let mut store = Store::new(
        &engine,
        State {
            limits: resources,
            io,
            remaining,
            claimed: false,
            live: false,
        },
    );
    store.limiter(|s| &mut s.limits);
    store
        .set_fuel(limits.fuel)
        .map_err(|e| BackendError(e.to_string()))?;
    let mut linker = Linker::new(&engine);
    const ABI: &str = "pactern.terminal.v1";
    linker
        .func_wrap(
            ABI,
            "root",
            |mut caller: Caller<'_, State<'_>>,
             slot: u32|
             -> std::result::Result<u32, wasmi::Error> {
                let state = caller.data_mut();
                if slot != 0 || state.claimed {
                    return Err(wasmi::Error::new("terminal root denied"));
                }
                state.claimed = true;
                state.live = true;
                Ok(1)
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    linker
        .func_wrap(
            ABI,
            "release",
            |mut caller: Caller<'_, State<'_>>,
             token: u32|
             -> std::result::Result<(), wasmi::Error> {
                caller.data().check(token)?;
                caller.data_mut().live = false;
                Ok(())
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    linker
        .func_wrap(
            ABI,
            "argc",
            |caller: Caller<'_, State<'_>>, token: u32| -> std::result::Result<u32, wasmi::Error> {
                caller.data().check(token)?;
                Ok(caller.data().io.arguments.len() as u32)
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    linker
        .func_wrap(
            ABI,
            "arg_len",
            |caller: Caller<'_, State<'_>>,
             token: u32,
             index: u32|
             -> std::result::Result<u32, wasmi::Error> {
                caller.data().check(token)?;
                caller
                    .data()
                    .io
                    .arguments
                    .get(index as usize)
                    .map(|a| a.len() as u32)
                    .ok_or_else(|| wasmi::Error::new("terminal argument index out of bounds"))
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    linker
        .func_wrap(
            ABI,
            "arg_copy",
            |mut caller: Caller<'_, State<'_>>,
             token: u32,
             index: u32,
             ptr: u32,
             len: u32|
             -> std::result::Result<(), wasmi::Error> {
                caller.data().check(token)?;
                caller.data_mut().charge(len as usize)?;
                let argument = caller
                    .data()
                    .io
                    .arguments
                    .get(index as usize)
                    .filter(|a| a.len() == len as usize)
                    .ok_or_else(|| wasmi::Error::new("invalid terminal argument copy"))?
                    .clone();
                memory(&caller)?
                    .write(&mut caller, ptr as usize, argument.as_bytes())
                    .map_err(io_error)
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    linker
        .func_wrap(
            ABI,
            "write",
            |mut caller: Caller<'_, State<'_>>,
             token: u32,
             stream: u32,
             ptr: u32,
             len: u32|
             -> std::result::Result<(), wasmi::Error> {
                caller.data().check(token)?;
                if stream > 1 {
                    return Err(wasmi::Error::new("invalid terminal stream"));
                }
                caller.data_mut().charge(len as usize)?;
                let mut bytes = vec![0; len as usize];
                memory(&caller)?
                    .read(&caller, ptr as usize, &mut bytes)
                    .map_err(io_error)?;
                std::str::from_utf8(&bytes).map_err(io_error)?;
                let state = caller.data_mut();
                let stream = if stream == 0 {
                    &mut state.io.output
                } else {
                    &mut state.io.error
                };
                stream.write_all(&bytes).map_err(io_error)?;
                stream.flush().map_err(io_error)
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    linker
        .func_wrap(
            ABI,
            "read_line",
            |mut caller: Caller<'_, State<'_>>,
             token: u32,
             ptr: u32,
             capacity: u32|
             -> std::result::Result<i32, wasmi::Error> {
                caller.data().check(token)?;
                if capacity > 65536 {
                    return Err(wasmi::Error::new("terminal line capacity exceeded"));
                }
                let memory = memory(&caller)?;
                if (ptr as usize)
                    .checked_add(capacity as usize)
                    .is_none_or(|end| end > memory.data(&caller).len())
                {
                    return Err(wasmi::Error::new("terminal input buffer out of bounds"));
                }
                // Bound allocations and consumed input before reading, including line endings.
                let limit = caller.data().remaining.min(capacity as usize + 2);
                let mut bytes = Vec::new();
                let mut eof = false;
                loop {
                    let state = caller.data_mut();
                    let available = state.io.input.fill_buf().map_err(io_error)?;
                    if available.is_empty() {
                        eof = true;
                        break;
                    }
                    let end = available.iter().position(|b| *b == b'\n').map(|n| n + 1);
                    let count = end
                        .unwrap_or(available.len())
                        .min(limit.saturating_sub(bytes.len()));
                    if count == 0 {
                        return Err(wasmi::Error::new("terminal line or byte limit exceeded"));
                    }
                    bytes.extend_from_slice(&available[..count]);
                    state.io.input.consume(count);
                    state.charge(count)?;
                    if bytes.last() == Some(&b'\n') {
                        break;
                    }
                }
                if eof && bytes.is_empty() {
                    return Ok(-1);
                }
                if bytes.last() == Some(&b'\n') {
                    bytes.pop();
                    if bytes.last() == Some(&b'\r') {
                        bytes.pop();
                    }
                }
                if bytes.len() > capacity as usize {
                    return Err(wasmi::Error::new("terminal line limit exceeded"));
                }
                std::str::from_utf8(&bytes).map_err(io_error)?;
                memory
                    .write(&mut caller, ptr as usize, &bytes)
                    .map_err(io_error)?;
                Ok(bytes.len() as i32)
            },
        )
        .map_err(|e| BackendError(e.to_string()))?;
    // Linker validation rejects spoofed function signatures and non-function imports.
    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .map_err(|e| BackendError(format!("terminal ABI instantiation failed: {e}")))?;
    super::execute_instance(store, instance, limits, 0, |_| Ok(()))
}
