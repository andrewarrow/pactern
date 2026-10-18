use crate::{CliCommand, source_commands};
use pactern::{
    Diagnostic,
    acceptance::TrustedBaseline,
    backend::{self, Execution, HostLimits},
    core::{self, DeclarationKind, Program},
    frontend::SourceSnapshot,
    interpreter::{Limits, Value},
    pacts,
    resolution::{ParsedFile, SymbolId, SymbolKind},
    semantic::{IdentityLedger, SemanticIndex},
    syntax::{SourceFile, parse},
    typing::ValueType,
    verification::{self, Status},
    workspace::{self, ChangeRequest, ManagedWorkspace, WorkspaceData},
};
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Diagnostic>;
fn tool(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::operation("PTN_TOOL_OPERATION", "tool", error.to_string())
}
fn semantic(error: pactern::semantic::SemanticError) -> Diagnostic {
    Diagnostic::operation(error.code, "change", error.message)
}
fn to_json(value: impl serde::Serialize) -> Result<Json> {
    serde_json::to_value(value).map_err(tool)
}
fn read(path: &Path) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path).map_err(tool)?;
    if !meta.is_file() || meta.len() > 32 * 1024 * 1024 {
        return Err(tool("expected a regular file no larger than 32 MiB"));
    }
    fs::read(path).map_err(tool)
}
fn load(
    paths: &[PathBuf],
    store: Option<&Path>,
    baseline: Option<&Path>,
) -> Result<SourceSnapshot> {
    if let Some(store) = store {
        let policy = policy(baseline)?;
        let snap = ManagedWorkspace::open(store)
            .map_err(semantic)?
            .snapshot(&policy)
            .map_err(semantic)?;
        workspace::source_snapshot(&snap.files).map_err(semantic)
    } else {
        source_commands::load(paths)
    }
}
fn program(snapshot: &SourceSnapshot) -> Result<Program> {
    if let Some(error) = snapshot.check().into_iter().next() {
        return Err(error);
    }
    let files = snapshot
        .inputs()
        .iter()
        .map(|i| {
            let s = SourceFile::from_bytes(&i.path, &i.bytes)
                .map_err(|e| snapshot.syntax_diagnostic(&i.path, &e))?;
            Ok(ParsedFile::new(
                &i.path,
                parse(&s, i.kind).map_err(|e| snapshot.syntax_diagnostic(&i.path, &e))?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    core::lower(&files).map_err(tool)
}
fn entry(p: &Program, name: Option<&str>) -> Result<SymbolId> {
    let functions = p
        .resolution()
        .symbols
        .iter()
        .filter(|s| s.kind == SymbolKind::Function && s.declaration.is_some())
        .collect::<Vec<_>>();
    let matches = functions
        .iter()
        .filter(|s| {
            name.map_or(s.name == "main", |name| {
                s.name == name
                    || s.module.is_some_and(|m| {
                        format!("{}::{}", p.resolution().modules[m.0].name, s.name) == name
                    })
            })
        })
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        return Ok(matches[0].id);
    }
    if name.is_none() && functions.len() == 1 {
        return Ok(functions[0].id);
    }
    Err(Diagnostic::invalid_arguments(
        "select one function with --entry MODULE::NAME",
    ))
}
struct UniqueJson(Json);
impl<'de> serde::Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("JSON with unique object keys")
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                value: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(Json::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(
                self,
                value: i64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(value.into()))
            }
            fn visit_u64<E: serde::de::Error>(
                self,
                value: u64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(value.into()))
            }
            fn visit_f64<E: serde::de::Error>(
                self,
                value: f64,
            ) -> std::result::Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueJson(Json::Number(number)))
                    .ok_or_else(|| E::custom("nonfinite JSON number"))
            }
            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(value.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(Json::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<UniqueJson>()? {
                    values.push(value.0);
                }
                Ok(UniqueJson(Json::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut object = serde_json::Map::new();
                while let Some((key, value)) = map.next_entry::<String, UniqueJson>()? {
                    if object.insert(key.clone(), value.0).is_some() {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate JSON key {key:?}"
                        )));
                    }
                }
                Ok(UniqueJson(Json::Object(object)))
            }
        }
        decoder.deserialize_any(Visitor)
    }
}
fn arguments(p: &Program, id: SymbolId, args: &str) -> Result<Vec<Value>> {
    let json: UniqueJson =
        serde_json::from_str(args).map_err(|e| Diagnostic::invalid_arguments(e.to_string()))?;
    let json = json.0;
    let args = json.as_array().ok_or_else(|| {
        Diagnostic::invalid_arguments("--args must be a JSON array; I64 values use decimal strings")
    })?;
    let DeclarationKind::Function {
        parameters,
        type_parameters,
        ..
    } = &p
        .declarations()
        .iter()
        .find(|d| d.symbol == id)
        .ok_or_else(|| tool("missing entry"))?
        .kind
    else {
        return Err(tool("entry must be a function"));
    };
    if !type_parameters.is_empty() || parameters.len() != args.len() {
        return Err(Diagnostic::invalid_arguments(
            "entry must be nongeneric and arguments must match its arity",
        ));
    }
    parameters
        .iter()
        .zip(args)
        .map(|(id, j)| decode(p, &p.symbol_types()[id], j, 0))
        .collect()
}
fn variant_shape(j: &Json, tag: &str, payload_key: Option<&str>) -> bool {
    if payload_key.is_none() && j.as_str() == Some(tag) {
        return true;
    }
    let Some(object) = j.as_object() else {
        return false;
    };
    object.get("tag").and_then(Json::as_str) == Some(tag)
        && object.len() == if payload_key.is_some() { 2 } else { 1 }
        && payload_key.is_none_or(|key| object.contains_key(key))
}
fn decode(p: &Program, t: &ValueType, j: &Json, depth: usize) -> Result<Value> {
    if depth > 64 {
        return Err(Diagnostic::invalid_arguments("argument nesting exceeds 64"));
    }
    let bad = || {
        Diagnostic::invalid_arguments(format!(
            "argument does not match {t:?}; I64 values require exact decimal strings"
        ))
    };
    Ok(match t {
        ValueType::Bool => Value::Bool(j.as_bool().ok_or_else(bad)?),
        ValueType::I64 => {
            let s = j.as_str().ok_or_else(bad)?;
            let n = s.parse::<i64>().map_err(|_| bad())?;
            if n.to_string() != s {
                return Err(bad());
            }
            Value::I64(n)
        }
        ValueType::String => Value::String(j.as_str().ok_or_else(bad)?.into()),
        ValueType::Unit if j.is_null() => Value::Unit,
        ValueType::List(t) => Value::List(
            j.as_array()
                .ok_or_else(bad)?
                .iter()
                .map(|j| decode(p, t, j, depth + 1))
                .collect::<Result<_>>()?,
        ),
        ValueType::Option(_) | ValueType::Result(_, _) => {
            let tag = j["tag"].as_str().or_else(|| j.as_str()).ok_or_else(bad)?;
            let (ty, key) = match (t, tag) {
                (ValueType::Option(inner), "Some") => (Some(inner.as_ref()), "value"),
                (ValueType::Option(_), "None") => (None, "value"),
                (ValueType::Result(inner, _), "Ok") => (Some(inner.as_ref()), "value"),
                (ValueType::Result(_, error), "Err") => (Some(error.as_ref()), "error"),
                _ => return Err(bad()),
            };
            if !variant_shape(j, tag, ty.map(|_| key)) {
                return Err(bad());
            }
            let variant = p
                .resolution()
                .symbols
                .iter()
                .find(|s| s.kind == SymbolKind::BuiltinVariant && s.name == tag)
                .ok_or_else(bad)?
                .id;
            Value::Variant {
                variant,
                payload: ty
                    .map(|ty| decode(p, ty, &j[key], depth + 1).map(|v| vec![v]))
                    .transpose()?
                    .unwrap_or_default(),
            }
        }
        ValueType::Named(id) | ValueType::Applied(id, _) => {
            let declaration = p
                .declarations()
                .iter()
                .find(|d| d.symbol == *id)
                .ok_or_else(bad)?;
            let args = if let ValueType::Applied(_, args) = t {
                args.as_slice()
            } else {
                &[]
            };
            match &declaration.kind {
                DeclarationKind::Struct {
                    type_parameters,
                    fields,
                } => {
                    let subs = type_parameters
                        .iter()
                        .copied()
                        .zip(args.iter().cloned())
                        .collect();
                    let object = j.as_object().ok_or_else(bad)?;
                    if object.len() != fields.len() {
                        return Err(bad());
                    }
                    Value::Record(
                        fields
                            .iter()
                            .map(|id| {
                                let name = &p.resolution().symbols[id.0].name;
                                Ok((
                                    *id,
                                    decode(
                                        p,
                                        &core::substitute(&p.symbol_types()[id], &subs),
                                        object.get(name).ok_or_else(bad)?,
                                        depth + 1,
                                    )?,
                                ))
                            })
                            .collect::<Result<_>>()?,
                    )
                }
                DeclarationKind::Enum {
                    type_parameters,
                    variants,
                } => {
                    let tag = j["tag"].as_str().or_else(|| j.as_str()).ok_or_else(bad)?;
                    let variant = variants
                        .iter()
                        .find(|v| p.resolution().symbols[v.symbol.0].name == tag)
                        .ok_or_else(bad)?;
                    let subs = type_parameters
                        .iter()
                        .copied()
                        .zip(args.iter().cloned())
                        .collect();
                    let payload_key = match variant.payload.len() {
                        0 => None,
                        1 => Some("value"),
                        _ => Some("values"),
                    };
                    if !variant_shape(j, tag, payload_key) {
                        return Err(bad());
                    }
                    let values = if variant.payload.is_empty() {
                        vec![]
                    } else if variant.payload.len() == 1 {
                        vec![&j["value"]]
                    } else {
                        j["values"].as_array().ok_or_else(bad)?.iter().collect()
                    };
                    if values.len() != variant.payload.len() {
                        return Err(bad());
                    }
                    Value::Variant {
                        variant: variant.symbol,
                        payload: variant
                            .payload
                            .iter()
                            .zip(values)
                            .map(|(t, j)| decode(p, &core::substitute(t, &subs), j, depth + 1))
                            .collect::<Result<_>>()?,
                    }
                }
                _ => return Err(bad()),
            }
        }
        _ => return Err(bad()),
    })
}
fn encode(p: &Program, v: &Value) -> Json {
    match v {
        Value::Bool(v) => json!(v),
        Value::I64(v) => json!(v.to_string()),
        Value::String(v) => json!(v),
        Value::Unit => Json::Null,
        Value::List(vs) => Json::Array(vs.iter().map(|v| encode(p, v)).collect()),
        Value::Record(fields) => Json::Object(
            fields
                .iter()
                .map(|(id, v)| (p.resolution().symbols[id.0].name.clone(), encode(p, v)))
                .collect(),
        ),
        Value::Variant { variant, payload } => {
            let name = &p.resolution().symbols[variant.0].name;
            match payload.as_slice() {
                [] => json!(name),
                [v] if name == "Err"
                    && p.resolution().symbols[variant.0].kind == SymbolKind::BuiltinVariant =>
                {
                    json!({"tag":name,"error":encode(p,v)})
                }
                [v] => json!({"tag":name,"value":encode(p,v)}),
                vs => {
                    json!({"tag":name,"values":vs.iter().map(|v|encode(p,v)).collect::<Vec<_>>()})
                }
            }
        }
    }
}
fn compiled(p: &Program, id: SymbolId, args: &[Value], guards: bool) -> Result<(String, Vec<u8>)> {
    let c = if guards {
        backend::emit_c_guarded(p, id, args, Limits::default())
    } else {
        backend::emit_c(p, id, args, Limits::default())
    }
    .map_err(tool)?;
    let wasm = backend::compile_wasm(&c).map_err(tool)?;
    Ok((c, wasm))
}
fn outcome(execution: Execution) -> pacts::Outcome {
    match execution {
        Execution::Value(v) => pacts::Outcome {
            value: Some(format!("{v:?}")),
            trap: None,
            cause: None,
        },
        Execution::Trap { kind, cause } => pacts::Outcome {
            value: None,
            trap: Some(format!("{kind:?}")),
            cause: cause.map(|c| format!("{c:?}")),
        },
    }
}
fn policy(path: Option<&Path>) -> Result<TrustedBaseline> {
    let path = path.ok_or_else(|| {
        Diagnostic::invalid_arguments(
            "managed operations require --baseline PATH to an externally trusted baseline",
        )
    })?;
    serde_json::from_slice(&read(path)?).map_err(tool)
}
fn index(
    paths: &[PathBuf],
    store: Option<&Path>,
    baseline: Option<&Path>,
) -> Result<SemanticIndex> {
    if let Some(store) = store {
        return Ok(ManagedWorkspace::open(store)
            .map_err(semantic)?
            .snapshot(&policy(baseline)?)
            .map_err(semantic)?
            .index);
    }
    let source = source_commands::load(paths)?;
    let ledger = Path::new(workspace::LEDGER_PATH);
    let ledger = if ledger.exists() {
        serde_json::from_slice(&read(ledger)?).map_err(tool)?
    } else {
        IdentityLedger {
            namespace: pactern::semantic::digest(format!("source-query/{}", source.revision())),
            next_id: 0,
            identities: vec![],
        }
    };
    SemanticIndex::build(&source, &ledger).map_err(semantic)
}
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(tool)?;
    }
    let temporary = path.with_extension(format!("pactern-tmp-{}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(tool)?;
    let result = (|| {
        file.write_all(bytes).map_err(tool)?;
        file.sync_all().map_err(tool)?;
        drop(file);
        fs::rename(&temporary, path).map_err(tool)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
fn resolved_output_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().len() > 4096 || path.components().count() > 64 {
        return Err(Diagnostic::invalid_arguments(
            "artifact path exceeds the supported size or nesting limit",
        ));
    }
    if let Ok(path) = fs::canonicalize(path) {
        return Ok(path);
    }
    if fs::symlink_metadata(path).is_ok() {
        return Err(tool("output path cannot be resolved safely"));
    }
    let name = path
        .file_name()
        .ok_or_else(|| Diagnostic::invalid_arguments("output path has no filename"))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(resolved_output_path(parent)?.join(name))
}
fn validate_artifact_output(
    output: &Path,
    source: &SourceSnapshot,
    store: Option<&Path>,
    baseline: Option<&Path>,
) -> Result<()> {
    let c_output = output.with_extension("c");
    if c_output == output
        || output
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("c"))
    {
        return Err(Diagnostic::invalid_arguments(
            "Wasm output and C sidecar must be different paths; use a .wasm output name",
        ));
    }
    let protected_store = store.map(fs::canonicalize).transpose().map_err(tool)?;
    let protected_baseline = baseline.map(fs::canonicalize).transpose().map_err(tool)?;
    let source_paths: Vec<_> = source
        .inputs()
        .iter()
        .filter_map(|input| fs::canonicalize(&input.path).ok())
        .collect();
    for target in [output, c_output.as_path()] {
        let target = resolved_output_path(target)?;
        if source_paths.contains(&target)
            || protected_store
                .as_ref()
                .is_some_and(|store| target.starts_with(store))
            || protected_baseline.as_ref() == Some(&target)
        {
            return Err(Diagnostic::invalid_arguments(
                "artifact output must not replace source, trusted policy, or managed workspace files",
            ));
        }
    }
    Ok(())
}
/// Return a report plus its process status; unknown proof never exits success.
pub fn run(
    command: &CliCommand,
    store: Option<&Path>,
    baseline: Option<&Path>,
) -> Result<Option<(Json, u8)>> {
    let result = match command {
        CliCommand::Verify { paths } => {
            let source = load(paths, store, baseline)?;
            let p = program(&source)?;
            let targets: BTreeSet<_> = p
                .declarations()
                .iter()
                .filter_map(|d| {
                    if let DeclarationKind::Pact { target, .. } = &d.kind {
                        Some(*target)
                    } else {
                        None
                    }
                })
                .collect();
            if targets.is_empty() {
                return Err(tool("no pacts to verify"));
            }
            let solver = verification::Solver::default();
            let proofs = targets
                .into_iter()
                .map(|id| verification::verify(&p, id, &solver))
                .collect::<Vec<_>>();
            let passed = proofs.iter().all(|p| {
                [
                    &p.precondition_consistency,
                    &p.postcondition,
                    &p.absence_of_traps,
                    &p.termination,
                ]
                .iter()
                .all(|c| c.status == Status::Proved)
            });
            (
                json!({"kind":"verification","protocol_version":pactern::PROTOCOL_VERSION,"source_revision":source.revision(),"inventory":pacts::inventory(&p,source.revision()),"proofs":proofs,"passed":passed}),
                u8::from(!passed),
            )
        }
        CliCommand::Build {
            terminal: true,
            paths,
            target,
            entry: name,
            args,
            output,
            no_contracts,
        }
        | CliCommand::Run {
            terminal: true,
            paths,
            target,
            entry: name,
            args,
            output,
            no_contracts,
            ..
        } => {
            if target != "wasm" || args.trim() != "[]" {
                return Err(Diagnostic::invalid_arguments(
                    "terminal entries use --target wasm and a single host-issued Terminal parameter; pass program arguments after --",
                ));
            }
            let source = load(paths, store, baseline)?;
            let p = program(&source)?;
            let id = entry(&p, name.as_deref())?;
            if !p.declarations().iter().any(|d| {
                d.symbol == id
                    && matches!(
                        &d.kind,
                        DeclarationKind::Function {
                            result: ValueType::I64 | ValueType::Unit,
                            ..
                        }
                    )
            }) {
                return Err(Diagnostic::invalid_arguments(
                    "terminal entry must return I64 (exit code 0..255) or Unit",
                ));
            }
            let (c, wasm, guards) = if *no_contracts {
                let c = backend::emit_c_terminal(&p, id, Limits::default(), false).map_err(tool)?;
                let wasm = backend::compile_wasm(&c).map_err(tool)?;
                (c, wasm, vec![])
            } else {
                let artifact =
                    pacts::compile_terminal_guarded(&p, source.revision(), id, Limits::default())
                        .map_err(tool)?;
                (artifact.c_source, artifact.wasm, artifact.guards)
            };
            let destination = if matches!(command, CliCommand::Build { .. }) {
                Some(
                    output
                        .as_deref()
                        .unwrap_or(Path::new("target/pactern.wasm")),
                )
            } else {
                output.as_deref()
            };
            if let Some(output) = destination {
                validate_artifact_output(output, &source, store, baseline)?;
                write(output, &wasm)?;
                write(&output.with_extension("c"), c.as_bytes())?;
            }
            if let CliCommand::Run { program_args, .. } = command {
                let stdin = std::io::stdin();
                let stdout = std::io::stdout();
                let stderr = std::io::stderr();
                let execution = backend::run_wasm_terminal(
                    &wasm,
                    HostLimits::default(),
                    backend::TerminalIo {
                        input: &mut stdin.lock(),
                        output: &mut stdout.lock(),
                        error: &mut stderr.lock(),
                        arguments: program_args,
                        byte_limit: 1024 * 1024,
                    },
                )
                .map_err(tool)?;
                let (outcome, status) = match execution {
                    Execution::Value(Value::I64(code)) => {
                        let status = u8::try_from(code)
                            .map_err(|_| tool("terminal exit code must be in 0..255"))?;
                        (json!({"kind":"return","value":code.to_string()}), status)
                    }
                    Execution::Value(Value::Unit) => (json!({"kind":"return","value":null}), 0),
                    Execution::Trap { kind, cause } => (
                        json!({"kind":"trap","trap":format!("{kind:?}"),"cause":cause.map(|c|format!("{c:?}"))}),
                        1,
                    ),
                    _ => return Err(tool("invalid terminal entry result")),
                };
                (json!({"kind":"execution","outcome":outcome}), status)
            } else {
                (
                    json!({"kind":"build","protocol_version":pactern::PROTOCOL_VERSION,"source_revision":source.revision(),"target":"wasm32-unknown-unknown","host":"terminal/1","artifact":destination,"artifact_hash":pactern::semantic::digest(&wasm),"artifact_bytes":wasm.len(),"compiler":backend::compiler_version().map_err(tool)?,"runtime_guards":!no_contracts,"guard_map":guards,"invocation_specialized":true}),
                    0,
                )
            }
        }
        CliCommand::Build {
            paths,
            target,
            entry: name,
            args,
            output,
            no_contracts,
            ..
        }
        | CliCommand::Run {
            paths,
            entry: name,
            args,
            output,
            target,
            no_contracts,
            ..
        } => {
            if matches!(command, CliCommand::Run { program_args, .. } if !program_args.is_empty()) {
                return Err(Diagnostic::invalid_arguments(
                    "program arguments after -- require --terminal",
                ));
            }
            if target != "wasm" {
                return Err(Diagnostic::invalid_arguments(
                    "only --target wasm is supported",
                ));
            }
            let source = load(paths, store, baseline)?;
            let p = program(&source)?;
            let id = entry(&p, name.as_deref())?;
            let args = arguments(&p, id, args)?;
            let (c, wasm, guards) = if *no_contracts {
                let (c, wasm) = compiled(&p, id, &args, false)?;
                (c, wasm, vec![])
            } else {
                let artifact =
                    pacts::compile_guarded(&p, source.revision(), id, &args, Limits::default())
                        .map_err(tool)?;
                (artifact.c_source, artifact.wasm, artifact.guards)
            };
            if matches!(command, CliCommand::Build { .. }) {
                let output = output
                    .as_deref()
                    .unwrap_or(Path::new("target/pactern.wasm"));
                validate_artifact_output(output, &source, store, baseline)?;
                write(output, &wasm)?;
                write(&output.with_extension("c"), c.as_bytes())?;
                (
                    json!({"kind":"build","protocol_version":pactern::PROTOCOL_VERSION,"source_revision":source.revision(),"target":"wasm32-unknown-unknown","artifact":output,"artifact_hash":pactern::semantic::digest(&wasm),"artifact_bytes":wasm.len(),"compiler":backend::compiler_version().map_err(tool)?,"runtime_guards":!no_contracts,"guard_map":guards,"invocation_specialized":true}),
                    0,
                )
            } else {
                if let Some(output) = output {
                    validate_artifact_output(output, &source, store, baseline)?;
                    write(output, &wasm)?;
                    write(&output.with_extension("c"), c.as_bytes())?;
                }
                let execution = backend::run_wasm(&wasm, HostLimits::default()).map_err(tool)?;
                let (value, status) = match execution {
                    Execution::Value(v) => (json!({"kind":"return","value":encode(&p,&v)}), 0),
                    Execution::Trap { kind, cause } => (
                        json!({"kind":"trap","trap":format!("{kind:?}"),"cause":cause.map(|c|format!("{c:?}"))}),
                        1,
                    ),
                };
                (
                    json!({"kind":"execution","protocol_version":pactern::PROTOCOL_VERSION,"source_revision":source.revision(),"artifact_hash":pactern::semantic::digest(&wasm),"runtime_guards":!no_contracts,"guard_map":guards,"outcome":value}),
                    status,
                )
            }
        }
        CliCommand::Test {
            paths,
            affected,
            entry: name,
            seed,
            cases,
        } => {
            let source = load(paths, store, baseline)?;
            let p = program(&source)?;
            let id = entry(&p, name.as_deref())?;
            let report = pacts::run_properties(
                &p,
                id,
                *seed,
                *cases,
                Limits::default(),
                "wasmi/2.0.0",
                |id, args| {
                    let (_, wasm) = compiled(&p, id, &args, true).map_err(|e| e.to_string())?;
                    backend::run_wasm(&wasm, HostLimits::default())
                        .map(outcome)
                        .map_err(|e| e.to_string())
                },
            )
            .map_err(tool)?;
            let status = u8::from(!report.passed());
            (
                json!({"kind":"tests","protocol_version":pactern::PROTOCOL_VERSION,"source_revision":source.revision(),"selection":if *affected{"all cases for selected entry; dependency filtering unavailable"}else{"entry properties"},"report":report}),
                status,
            )
        }
        CliCommand::Editor { path, byte } => match (path, byte) {
            (None, None) => {
                let source = load(&[], store, baseline)?;
                (
                    to_json(pactern::editor::diagnostics(&source).map_err(semantic)?)?,
                    0,
                )
            }
            (Some(path), Some(byte)) => {
                let i = index(&[], store, baseline)?;
                (
                    to_json(
                        pactern::editor::definition(&i, &i.source_revision, path, *byte)
                            .map_err(semantic)?,
                    )?,
                    0,
                )
            }
            _ => {
                return Err(Diagnostic::invalid_arguments(
                    "definition requires both --path and --byte",
                ));
            }
        },
        CliCommand::Inspect { symbol } => {
            let symbol = symbol
                .as_deref()
                .ok_or_else(|| Diagnostic::invalid_arguments("missing SYMBOL"))?;
            let i = index(&[], store, baseline)?;
            (to_json(i.inspect(symbol).map_err(semantic)?)?, 0)
        }
        CliCommand::Callers { symbol } => {
            let symbol = symbol
                .as_deref()
                .ok_or_else(|| Diagnostic::invalid_arguments("missing SYMBOL"))?;
            let i = index(&[], store, baseline)?;
            (to_json(i.callers(symbol).map_err(semantic)?)?, 0)
        }
        CliCommand::Impact { change } => {
            let path = change
                .as_deref()
                .ok_or_else(|| Diagnostic::invalid_arguments("missing --change PATH"))?;
            let change = ChangeRequest::decode(&read(path)?).map_err(semantic)?;
            let i = index(&[], store, baseline)?;
            if i.source_revision != change.base_revision {
                return Err(Diagnostic::operation(
                    "PTN_CHANGE_STALE_REVISION",
                    "change",
                    "impact request is stale",
                ));
            }
            (
                to_json(
                    i.impact(
                        &change
                            .operations
                            .iter()
                            .map(|op| op.target_id().to_owned())
                            .collect::<Vec<_>>(),
                    )
                    .map_err(semantic)?,
                )?,
                0,
            )
        }
        CliCommand::Apply { change, dry_run } => {
            let path = change
                .as_deref()
                .ok_or_else(|| Diagnostic::invalid_arguments("missing CHANGE"))?;
            let request = ChangeRequest::decode(&read(path)?).map_err(semantic)?;
            let store_path = store.ok_or_else(|| {
                Diagnostic::invalid_arguments(
                    "apply requires --store PATH for an atomic managed workspace",
                )
            })?;
            let workspace = ManagedWorkspace::open(store_path).map_err(semantic)?;
            let baseline_path = baseline;
            let baseline = policy(baseline_path)?;
            let staged = workspace
                .stage_with(&baseline, &request, |candidate| {
                    let source = workspace::source_snapshot(&candidate.files)?;
                    let p = program(&source).map_err(|e| {
                        pactern::semantic::SemanticError::new(e.code(), e.message())
                    })?;
                    let targets: BTreeSet<_> = p
                        .declarations()
                        .iter()
                        .filter_map(|d| {
                            if let DeclarationKind::Pact { target, .. } = &d.kind {
                                Some(*target)
                            } else {
                                None
                            }
                        })
                        .collect();
                    for target in targets {
                        let proof =
                            verification::verify(&p, target, &verification::Solver::default());
                        if [
                            &proof.precondition_consistency,
                            &proof.postcondition,
                            &proof.absence_of_traps,
                            &proof.termination,
                        ]
                        .iter()
                        .any(|c| c.status != Status::Proved)
                        {
                            return Err(pactern::semantic::SemanticError::new(
                                "PTN_POLICY_REQUIRED_PROOF",
                                "candidate lacks required proof for every pact",
                            ));
                        }
                    }
                    Ok(BTreeSet::from(["verification".into()]))
                })
                .map_err(semantic)?;
            if *dry_run {
                (
                    json!({"kind":"change_preview","receipt":staged.preview()}),
                    0,
                )
            } else {
                let receipt = workspace
                    .commit(&policy(baseline_path)?, staged, None)
                    .map_err(semantic)?;
                (json!({"kind":"change_committed","receipt":receipt}), 0)
            }
        }
        CliCommand::Init { paths } => {
            let store =
                store.ok_or_else(|| Diagnostic::invalid_arguments("init requires --store PATH"))?;
            let baseline = baseline.ok_or_else(|| {
                Diagnostic::invalid_arguments(
                    "init requires --baseline PATH outside the candidate tree",
                )
            })?;
            let parent = baseline.parent().unwrap_or(Path::new("."));
            let parent = fs::canonicalize(parent).map_err(tool)?;
            let root = fs::canonicalize(std::env::current_dir().map_err(tool)?).map_err(tool)?;
            if parent.starts_with(root) {
                return Err(Diagnostic::invalid_arguments(
                    "baseline must be outside the candidate tree",
                ));
            }
            if baseline.exists() || store.exists() {
                return Err(tool(
                    "init refuses to replace an existing store or baseline",
                ));
            }
            let source = source_commands::load(paths)?;
            let files = source
                .inputs()
                .iter()
                .map(|i| (i.path.clone(), i.bytes.clone()))
                .collect();
            let data = WorkspaceData::new(files, vec![]).map_err(semantic)?;
            let i = SemanticIndex::build(&source, &data.ledger).map_err(semantic)?;
            let mut policy =
                TrustedBaseline::capture(&data.files, &i, &[], &[], vec![]).map_err(semantic)?;
            policy.require_check("verification");
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(baseline)
                .map_err(tool)?;
            file.write_all(&serde_json::to_vec_pretty(&policy).map_err(tool)?)
                .map_err(tool)?;
            file.sync_all().map_err(tool)?;
            let managed = ManagedWorkspace::create(store, &data).map_err(semantic)?;
            let snapshot = managed.snapshot(&policy).map_err(semantic)?;
            (
                json!({"kind":"managed_workspace","source_revision":snapshot.revision,"policy_baseline":snapshot.policy_baseline,"store":store,"baseline":baseline,"declarations":snapshot.index.declarations}),
                0,
            )
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}
