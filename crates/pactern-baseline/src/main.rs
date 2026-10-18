use pactern_baseline::{Corpus, Error, Response, Tasks, compare, read_json, source_text};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
struct Measurement {
    language: String,
    tool_version: String,
    build_command: Vec<String>,
    build_ns: u128,
    artifact_bytes: u64,
    process_samples_ns: Vec<u128>,
    min_ns: u128,
    median_ns: u128,
    p95_ns: u128,
    max_ns: u128,
    stdin_bytes: usize,
    stdout_bytes: usize,
    outcomes: Vec<Response>,
}

#[derive(Serialize)]
struct Report {
    format_version: &'static str,
    kind: &'static str,
    pactern_execution: &'static str,
    unix_time_seconds: u64,
    os: &'static str,
    arch: &'static str,
    parallelism: usize,
    samples: usize,
    warmups: usize,
    case_count: usize,
    measurement_scope: &'static str,
    build_cache_policy: &'static str,
    agent_costs: &'static str,
    input_files: BTreeMap<String, String>,
    measurements: Vec<Measurement>,
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn checked_output(command: &mut Command) -> Result<Vec<u8>, Error> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "{command:?} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output.stdout)
}

fn run_program(binary: &Path, input: &[u8]) -> Result<Vec<u8>, Error> {
    let mut child = Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Drain stdout while writing stdin, so growing fixture batches cannot fill
    // both pipes and deadlock the harness.
    let mut stdin = child.stdin.take().ok_or("missing child stdin")?;
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(input));
        let output = child.wait_with_output()?;
        writer.join().map_err(|_| "stdin writer panicked")??;
        if !output.status.success() {
            return Err(format!(
                "{} failed: {}",
                binary.display(),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(output.stdout)
    })
}

struct Workload<'a> {
    input: &'a [u8],
    expected: &'a [Response],
    samples: usize,
}

fn measure(
    language: &str,
    version: String,
    build_command: Vec<String>,
    command: &mut Command,
    binary: &Path,
    workload: &Workload<'_>,
) -> Result<Measurement, Error> {
    let Workload {
        input,
        expected,
        samples,
    } = *workload;
    let start = Instant::now();
    checked_output(command)?;
    let build_ns = start.elapsed().as_nanos();
    let artifact_bytes = fs::metadata(binary)?.len();
    compare(expected, &run_program(binary, input)?)?;
    let mut process_samples_ns = Vec::with_capacity(samples);
    let mut outcomes = Vec::new();
    let mut stdout_bytes = 0;
    for _ in 0..samples {
        let start = Instant::now();
        let output = run_program(binary, input)?;
        process_samples_ns.push(start.elapsed().as_nanos());
        // Decode and compare outside the measured interval on every sample.
        outcomes = compare(expected, &output)?;
        stdout_bytes = output.len();
    }
    let mut sorted = process_samples_ns.clone();
    sorted.sort_unstable();
    Ok(Measurement {
        language: language.to_owned(),
        tool_version: version,
        build_command,
        build_ns,
        artifact_bytes,
        min_ns: sorted[0],
        median_ns: sorted[(samples - 1) / 2],
        p95_ns: sorted[(samples * 95).div_ceil(100) - 1],
        max_ns: sorted[samples - 1],
        process_samples_ns,
        stdin_bytes: input.len(),
        stdout_bytes,
        outcomes,
    })
}

fn run() -> Result<(), Error> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let mut output = root.join("target/baseline.json");
    let mut samples = 5;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--samples" => {
                samples = args
                    .next()
                    .ok_or("--samples requires a value")?
                    .parse::<usize>()?;
            }
            "--output" => {
                output = PathBuf::from(args.next().ok_or("--output requires a path")?);
            }
            "--help" | "-h" => {
                println!(
                    "Usage: pactern-baseline [--samples 1..1000] [--output PATH]\nValidates the corpus, builds Go/Rust baselines, checks every result, and records costs."
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }
    if !(1..=1000).contains(&samples) {
        return Err("--samples must be between 1 and 1000".into());
    }
    let corpus_root = root.join("fixtures/conformance");
    let corpus: Corpus = read_json(&corpus_root.join("manifest.json"))?;
    corpus.validate(&corpus_root)?;
    let tasks: Tasks = read_json(&root.join("baselines/tasks.json"))?;
    let (requests, expected) = tasks.requests(&corpus)?;
    let input = serde_json::to_vec(&requests)?;
    let workload = Workload {
        input: &input,
        expected: &expected,
        samples,
    };
    let mut input_files = BTreeMap::new();
    for path in [
        "baselines/tasks.json",
        "baselines/rust/main.rs",
        "baselines/go/main.go",
        "fixtures/conformance/manifest.json",
        "Cargo.lock",
        "Cargo.toml",
        "crates/pactern-baseline/Cargo.toml",
        "crates/pactern-baseline/src/lib.rs",
        "crates/pactern-baseline/src/main.rs",
        "rust-toolchain.toml",
    ] {
        input_files.insert(path.to_owned(), fs::read_to_string(root.join(path))?);
    }
    for case in &corpus.cases {
        for path in std::iter::once(&case.source).chain(&case.contracts) {
            input_files.insert(
                format!("fixtures/conformance/{path}"),
                source_text(&corpus_root, path)?,
            );
        }
    }
    // Resolve both prerequisites before spending time compiling either baseline.
    let rust_version = String::from_utf8(checked_output(Command::new("rustc").arg("--version"))?)?
        .trim()
        .to_owned();
    let go_version = String::from_utf8(checked_output(Command::new("go").arg("version"))?)?
        .trim()
        .to_owned();
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let scratch = Scratch(root.join("target").join(format!(
        "baseline-{}-{}",
        std::process::id(),
        now.as_nanos()
    )));
    fs::create_dir_all(&scratch.0)?;
    let rust_target = scratch.0.join("rust-target");
    let rust_binary = rust_target
        .join("release")
        .join(format!("baseline-rust{}", std::env::consts::EXE_SUFFIX));
    let go_binary = scratch
        .0
        .join(format!("baseline-go{}", std::env::consts::EXE_SUFFIX));
    let rust_args = [
        "build",
        "--release",
        "--locked",
        "-p",
        "pactern-baseline",
        "--bin",
        "baseline-rust",
        "--target-dir",
    ];
    eprintln!(
        "Checking {} conformance records; measuring {} baseline cases ({samples} samples per language).",
        corpus.cases.len(),
        expected.len()
    );
    let rust = measure(
        "rust",
        rust_version,
        std::iter::once("cargo")
            .chain(rust_args)
            .chain(["<fresh>/rust-target"])
            .map(str::to_owned)
            .collect(),
        Command::new("cargo")
            .args(rust_args)
            .arg(&rust_target)
            .current_dir(&root),
        &rust_binary,
        &workload,
    )?;
    eprintln!("Rust: {} expected outcomes matched.", rust.outcomes.len());
    let go = measure(
        "go",
        go_version,
        [
            "go",
            "build",
            "-trimpath",
            "-o",
            "<fresh>/baseline-go",
            "./baselines/go/main.go",
        ]
        .map(str::to_owned)
        .to_vec(),
        Command::new("go")
            .args(["build", "-trimpath", "-o"])
            .arg(&go_binary)
            .arg("./baselines/go/main.go")
            .env("GOCACHE", scratch.0.join("go-cache"))
            .current_dir(&root),
        &go_binary,
        &workload,
    )?;
    eprintln!("Go: {} expected outcomes matched.", go.outcomes.len());
    let report = Report {
        format_version: "0.1.0",
        kind: "baseline_measurement",
        pactern_execution: "not_implemented",
        unix_time_seconds: now.as_secs(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        parallelism: std::thread::available_parallelism()?.get(),
        samples,
        warmups: 1,
        case_count: expected.len(),
        measurement_scope: "Each sample is a fresh process running the entire case batch; includes startup, pipes, JSON decoding/encoding, and computation. Harness comparison is excluded. Not per-function latency or throughput.",
        build_cache_policy: "Fresh Rust target directory and Go GOCACHE per run; dependency downloads, OS caches, linker/toolchain caches, and machine load are uncontrolled. Build times include dependencies (Rust) and standard library (Go).",
        agent_costs: "Not measured: no model, token, repair, or human-review trial has run. These public seed cases are not a held-out evaluation.",
        input_files,
        measurements: vec![rust, go],
    };
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut json = serde_json::to_vec_pretty(&report)?;
    json.push(b'\n');
    fs::write(&output, json)?;
    eprintln!("Wrote {}", output.display());
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("baseline failed: {error}");
        std::process::exit(1);
    }
}
