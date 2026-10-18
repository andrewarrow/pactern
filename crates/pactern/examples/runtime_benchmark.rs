//! Reproducible process-level smoke measurement; not an agent repair experiment.
use pactern::{
    backend::{self, Execution, HostLimits},
    core::{self, DeclarationKind},
    interpreter::{self, Limits, Value},
    resolution::{ParsedFile, SymbolKind},
    syntax::{FileKind, SourceFile, parse},
    typing::ValueType,
};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::Instant,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn sha(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn decode(t: &ValueType, j: &Json) -> Value {
    match t {
        ValueType::I64 => Value::I64(j.as_str().unwrap().parse().unwrap()),
        ValueType::String => Value::String(j.as_str().unwrap().into()),
        ValueType::Bool => Value::Bool(j.as_bool().unwrap()),
        ValueType::List(t) => {
            Value::List(j.as_array().unwrap().iter().map(|j| decode(t, j)).collect())
        }
        _ => panic!("unsupported benchmark input {t:?}"),
    }
}
fn encode(names: &[String], v: Value) -> Json {
    match v {
        Value::Unit => Json::Null,
        Value::Bool(v) => json!(v),
        Value::I64(v) => json!(v.to_string()),
        Value::String(v) => json!(v),
        Value::List(vs) => Json::Array(vs.into_iter().map(|v| encode(names, v)).collect()),
        Value::Record(vs) => Json::Object(
            vs.into_iter()
                .map(|(id, v)| (names[id.0].clone(), encode(names, v)))
                .collect(),
        ),
        Value::Variant {
            variant,
            mut payload,
        } => {
            let name = &names[variant.0];
            if payload.is_empty() {
                json!(name)
            } else {
                let v = encode(names, payload.remove(0));
                if name == "Err" {
                    json!({"tag":name,"error":v})
                } else {
                    json!({"tag":name,"value":v})
                }
            }
        }
    }
}
fn outcome(names: &[String], e: Execution) -> Json {
    match e {
        Execution::Value(v) => json!({"kind":"return","value":encode(names,v)}),
        Execution::Trap { kind, cause } => {
            json!({"kind":"trap","category":format!("{kind:?}"),"cause":cause.map(|c|format!("{c:?}"))})
        }
    }
}
fn worker() -> Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let requests: Json = serde_json::from_str(&input)?;
    let mut results = vec![];
    for request in requests.as_array().ok_or("expected requests array")? {
        let names: Vec<String> = serde_json::from_value(request["symbols"].clone())?;
        let wasm = std::fs::read(request["artifact"].as_str().ok_or("missing artifact")?)?;
        let executed = backend::run_wasm(&wasm, HostLimits::default())?;
        results.push(json!({"id":request["id"],"outcome":outcome(&names,executed)}));
    }
    serde_json::to_writer(std::io::stdout(), &results)?;
    Ok(())
}
fn run_process(input: &[u8]) -> Result<Vec<u8>> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("missing stdin")?;
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(input));
        let output = child.wait_with_output()?;
        writer.join().map_err(|_| "writer panic")??;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        Ok(output.stdout)
    })
}
fn run() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--worker") {
        return worker();
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/candidate-pactern.json".into());
    let fixture_root = root.join("fixtures/conformance");
    let manifest: Json =
        serde_json::from_slice(&std::fs::read(fixture_root.join("manifest.json"))?)?;
    let tasks: Json = serde_json::from_slice(&std::fs::read(root.join("baselines/tasks.json"))?)?;
    let cases = manifest["cases"].as_array().ok_or("missing cases")?;
    let artifact_root = root.join("target/candidate-runtime-artifacts");
    std::fs::create_dir_all(&artifact_root)?;
    let mut measurements = vec![];
    let mut artifacts = vec![];
    let mut hashes = BTreeMap::new();
    for guarded in [false, true] {
        let build_start = Instant::now();
        let mut requests = vec![];
        let mut expected = vec![];
        let mut artifact_bytes = 0;
        for task in tasks["tasks"].as_array().ok_or("missing tasks")? {
            for case_id in task["case_ids"].as_array().ok_or("missing case ids")? {
                let case = cases
                    .iter()
                    .find(|c| c["id"] == *case_id)
                    .ok_or("unknown case")?;
                let mut parsed = vec![];
                let source_path = case["source"].as_str().ok_or("source")?;
                let text = std::fs::read_to_string(fixture_root.join(source_path))?;
                hashes.insert(
                    format!("fixtures/conformance/{source_path}"),
                    sha(text.as_bytes()),
                );
                parsed.push(ParsedFile::new(
                    source_path,
                    parse(
                        &SourceFile::new(source_path, &text),
                        FileKind::Implementation,
                    )?,
                ));
                if let Some(contracts) = case["contracts"].as_array() {
                    for path in contracts {
                        let path = path.as_str().ok_or("contract path")?;
                        let text = std::fs::read_to_string(fixture_root.join(path))?;
                        hashes.insert(format!("fixtures/conformance/{path}"), sha(text.as_bytes()));
                        parsed.push(ParsedFile::new(
                            path,
                            parse(&SourceFile::new(path, &text), FileKind::Contract)?,
                        ));
                    }
                }
                let p = core::lower(&parsed)?;
                let name = case["entry"]
                    .as_str()
                    .ok_or("entry")?
                    .rsplit("::")
                    .next()
                    .ok_or("entry name")?;
                let f = p
                    .resolution()
                    .symbols
                    .iter()
                    .find(|s| s.kind == SymbolKind::Function && s.name == name)
                    .ok_or("function")?
                    .id;
                let DeclarationKind::Function { parameters, .. } = &p
                    .declarations()
                    .iter()
                    .find(|d| d.symbol == f)
                    .ok_or("declaration")?
                    .kind
                else {
                    return Err("not function".into());
                };
                let args: Vec<_> = parameters
                    .iter()
                    .zip(case["args"].as_array().ok_or("args")?)
                    .map(|(id, j)| decode(&p.symbol_types()[id], j))
                    .collect();
                let c = if guarded {
                    backend::emit_c_guarded(&p, f, &args, Limits::default())?
                } else {
                    backend::emit_c(&p, f, &args, Limits::default())?
                };
                let wasm = backend::compile_wasm(&c)?;
                artifact_bytes += wasm.len();
                let path = artifact_root.join(format!(
                    "{}-{}.wasm",
                    if guarded { "guarded" } else { "raw" },
                    case_id.as_str().ok_or("id")?
                ));
                std::fs::write(&path, &wasm)?;
                let names: Vec<_> = p
                    .resolution()
                    .symbols
                    .iter()
                    .map(|s| s.name.clone())
                    .collect();
                let reference = if guarded {
                    interpreter::execute_guarded(&p, f, args, Limits::default())
                } else {
                    interpreter::execute(&p, f, args, Limits::default())
                };
                let reference = match reference {
                    Ok(v) => Execution::Value(v),
                    Err(t) => Execution::Trap {
                        kind: t.kind,
                        cause: t.cause.map(|t| t.kind),
                    },
                };
                expected.push(json!({"id":case_id,"outcome":outcome(&names,reference)}));
                requests.push(json!({"id":case_id,"artifact":path,"symbols":names}));
                artifacts.push(json!({"id":case_id,"guarded":guarded,"sha256":sha(&wasm),"bytes":wasm.len(),"c_sha256":sha(c.as_bytes())}));
            }
        }
        let build_ns = build_start.elapsed().as_nanos();
        let input = serde_json::to_vec(&requests)?;
        let wanted = json!(expected);
        let warmup: Json = serde_json::from_slice(&run_process(&input)?)?;
        if warmup != wanted {
            return Err("warmup differs from interpreter".into());
        }
        let mut samples = vec![];
        for _ in 0..5 {
            let start = Instant::now();
            let bytes = run_process(&input)?;
            samples.push(start.elapsed().as_nanos());
            let actual: Json = serde_json::from_slice(&bytes)?;
            if actual != wanted {
                return Err("measured outcome differs from interpreter".into());
            }
        }
        let mean = samples.iter().sum::<u128>() as f64 / samples.len() as f64;
        let variance = samples
            .iter()
            .map(|s| (*s as f64 - mean).powi(2))
            .sum::<f64>()
            / (samples.len() - 1) as f64;
        let mut sorted = samples.clone();
        sorted.sort_unstable();
        measurements.push(json!({"language":"pactern","contracts":if guarded{"enabled"}else{"disabled"},"case_count":requests.len(),"build_ns":build_ns,"artifact_bytes":artifact_bytes,"process_samples_ns":samples,"median_ns":sorted[2],"sample_variance_ns2":variance,"outcomes":expected}));
    }
    for path in [
        "Cargo.lock",
        "runtime/pactern.c",
        "runtime/quota.c",
        "crates/pactern/src/backend.rs",
        "crates/pactern/src/backend_quota.rs",
        "crates/pactern/src/core.rs",
        "crates/pactern/src/host.rs",
        "crates/pactern/src/interpreter.rs",
        "crates/pactern/examples/runtime_benchmark.rs",
        "fixtures/conformance/manifest.json",
        "baselines/tasks.json",
    ] {
        hashes.insert(path.to_string(), sha(&std::fs::read(root.join(path))?));
    }
    let report = json!({"format_version":"0.1.0","kind":"candidate_runtime_smoke","unix_time_seconds":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs(),"tool_version":pactern::TOOL_VERSION,"compiler":backend::compiler_version()?,"host":"wasmi 2.0.0 portable-dispatch","os":std::env::consts::OS,"arch":std::env::consts::ARCH,"samples":5,"warmups":1,"measurement_scope":"Fresh process for same 25 public seed cases as Go/Rust; includes JSON IO, reading invocation artifacts, Wasm validation/translation/instantiation and computation. C compilation excluded. Artifacts specialize each input; these are smoke costs, not comparable native function latency or a performance ranking.","build_scope":"Warm compiler and dependency caches; includes source checking, C emission and C/Wasm compilation of 25 input-bound artifacts. Does not include building the Rust host executable. Different scope from cold Go/Rust build measurements.","agent_experiment":{"status":"not_run","text_vs_semantic_edit":"unmeasured","matched_models_budgets_documentation":"unmeasured","token_currency_costs":"unmeasured","repair_success":"unmeasured","review_time":"unmeasured"},"decision":"narrow","source_hashes":hashes,"artifacts":artifacts,"measurements":measurements});
    let output = Path::new(&output);
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        output,
        format!("{}\n", serde_json::to_string_pretty(&report)?),
    )?;
    println!("Wrote {}", output.display());
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("runtime benchmark: {error}");
        std::process::exit(1)
    }
}
