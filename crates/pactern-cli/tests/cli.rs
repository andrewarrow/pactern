use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

const CLI: &str = env!("CARGO_BIN_EXE_pactern");

fn run(args: &[&str]) -> Output {
    Command::new(CLI).args(args).output().expect("run pactern")
}

fn run_in(directory: &Path, args: &[&str]) -> Output {
    Command::new(CLI)
        .current_dir(directory)
        .args(args)
        .output()
        .expect("run pactern")
}

fn json_output(args: &[&str], code: i32) -> Value {
    let output = run(args);
    assert_eq!(output.status.code(), Some(code), "args: {args:?}");
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout was not exactly one JSON document ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn assert_diagnostic(args: &[&str], code: &str, exit: i32) -> Value {
    let value = json_output(args, exit);
    assert_eq!(value["kind"], "diagnostic");
    assert_eq!(value["protocol_version"], "0.1.0");
    assert_eq!(value["code"], code);
    assert_eq!(value["severity"], "error");
    assert_eq!(value["phase"], "tool");
    assert!(
        value["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );
    value
}

#[test]
fn help_no_arguments_and_subcommand_help_succeed() {
    let plain = run(&[]);
    assert_eq!(plain.status.code(), Some(0));
    assert!(plain.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&plain.stdout).trim().is_empty());

    let help = json_output(&["--json", "--help"], 0);
    assert_eq!(help["kind"], "help");
    assert_eq!(help["cli_version"], "0.1.0");
    assert!(help["text"].as_str().is_some_and(|text| !text.is_empty()));

    for args in [
        vec!["check", "--json", "--help"],
        vec!["fmt", "--help", "--json"],
        vec!["verify", "--help", "--json"],
        vec!["test", "--help", "--json"],
        vec!["build", "--help", "--json"],
        vec!["inspect", "--json", "--help"],
        vec!["callers", "--help", "--json"],
        vec!["impact", "--help", "--json"],
        vec!["apply", "--json", "--help"],
        vec!["help", "inspect", "--json"],
    ] {
        let value = json_output(&args, 0);
        assert_eq!(value["kind"], "help", "args: {args:?}");
    }
}

#[test]
fn version_aliases_return_version_envelope() {
    for args in [vec!["--version"], vec!["-V"], vec!["version"]] {
        let value = json_output(&[&["--json"][..], &args[..]].concat(), 0);
        assert_eq!(value["kind"], "version");
        assert_eq!(value["cli_version"], "0.1.0");
        assert_eq!(value["tool_version"], "0.1.0");
        assert_eq!(value["language_version"], "0.1.0");
        assert_eq!(value["protocol_version"], "0.1.0");
    }
}

#[test]
fn help_and_version_before_subcommands_bypass_required_operands() {
    for command in ["inspect", "callers", "impact", "apply"] {
        for (flag, kind) in [("--help", "help"), ("--version", "version")] {
            let before = json_output(&["--json", flag, command], 0);
            let after = json_output(&[command, flag, "--json"], 0);
            assert_eq!(before["kind"], kind);
            assert_eq!(before, after, "flag position changed output for {command}");
        }
    }
}

#[test]
fn json_is_global_and_errors_use_one_diagnostic_document() {
    for args in [
        vec!["--json", "check"],
        vec!["check", "--json"],
        vec!["--json", "fmt", "--check"],
        vec!["test", "--affected", "--json"],
        vec!["build", "--target", "unknown", "--json"],
    ] {
        let value = json_output(
            &args,
            if args.windows(2).any(|w| w == ["--target", "unknown"]) {
                2
            } else {
                1
            },
        );
        assert_eq!(value["kind"], "diagnostic", "args: {args:?}");
    }

    assert_diagnostic(&["--json", "--unknown"], "PTN_TOOL_INVALID_ARGUMENTS", 2);
    assert_diagnostic(&["--json", "nonesuch"], "PTN_TOOL_INVALID_ARGUMENTS", 2);
    assert_diagnostic(&["inspect", "--json"], "PTN_TOOL_INVALID_ARGUMENTS", 2);
    assert_diagnostic(&["callers", "--json"], "PTN_TOOL_INVALID_ARGUMENTS", 2);
    assert_diagnostic(&["impact", "--json"], "PTN_TOOL_INVALID_ARGUMENTS", 2);
    assert_diagnostic(&["apply", "--json"], "PTN_TOOL_INVALID_ARGUMENTS", 2);
}

#[test]
fn unsupported_protocol_wins_even_for_help_and_version() {
    for args in [
        vec!["--json", "--protocol-version", "9.9", "--help"],
        vec!["version", "--json", "--protocol-version", "9.9"],
    ] {
        assert_diagnostic(&args, "PTN_TOOL_UNSUPPORTED_PROTOCOL", 2);
    }
}

#[test]
fn terminator_disables_global_json_detection() {
    let output = run(&["--", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!output.stderr.is_empty());
    assert!(serde_json::from_slice::<Value>(&output.stdout).is_err());
}

#[cfg(unix)]
#[test]
fn invalid_utf8_argument_is_reported_as_json_diagnostic() {
    use std::os::unix::ffi::OsStringExt;

    let output = Command::new(CLI)
        .arg("--json")
        .arg(OsString::from_vec(vec![b'-', b'-', 0xff]))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["kind"], "diagnostic");
    assert_eq!(value["code"], "PTN_TOOL_INVALID_ARGUMENTS");
}

#[test]
fn invalid_project_commands_accept_unicode_paths_without_touching_project() {
    let temp = TempDir::new();
    let project = temp.path().join("pactern-🧪");
    fs::create_dir_all(project.join("nested")).unwrap();
    fs::write(project.join("pactern.toml"), b"source = \"unchanged\"\n").unwrap();
    fs::write(project.join("nested\u{1f9ea}.txt"), [0, 1, 2, 255]).unwrap();
    let unicode_change = project.join("change-🧪.json");
    fs::write(&unicode_change, b"not parsed by the milestone skeleton").unwrap();
    let missing_change = project.join("missing-🧪.json");
    let before = tree_snapshot(&project);

    for args in [
        vec!["verify", "--json"],
        vec!["test", "--affected", "--json"],
        vec!["build", "--target", "wasm", "--json"],
        vec!["inspect", "Symbol", "--json"],
        vec!["callers", "Symbol", "--json"],
    ] {
        let output = run_in(&project, &args);
        assert_eq!(output.status.code(), Some(1), "args: {args:?}");
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["code"], "PTN_TOOL_NO_SOURCES", "args: {args:?}");
    }

    for path in [&unicode_change, &missing_change] {
        let mut impact_args: Vec<OsString> =
            vec!["--json".into(), "impact".into(), "--change".into()];
        impact_args.push(path.as_os_str().to_owned());
        assert_project_error(run_os_in(&project, impact_args));

        for dry_run in [false, true] {
            let mut apply_args: Vec<OsString> = vec!["--json".into(), "apply".into()];
            apply_args.push(path.as_os_str().to_owned());
            if dry_run {
                apply_args.push("--dry-run".into());
            }
            assert_project_error(run_os_in(&project, apply_args));
        }
    }
    assert_eq!(tree_snapshot(&project), before);
}

#[test]
fn check_loads_a_closed_source_package_and_reports_located_errors() {
    let temp = TempDir::new();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::create_dir(temp.path().join("contracts")).unwrap();
    let source = temp.path().join("src/main.ptn");
    fs::write(&source, "module main; fn answer()->I64 {return 42;}").unwrap();
    fs::write(
        temp.path().join("contracts/main.pact.ptn"),
        "module main; pact positive for answer {ensures result > 0;}",
    )
    .unwrap();
    let before = tree_snapshot(temp.path());
    let output = run_in(temp.path(), &["check", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(output.stderr.is_empty());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["kind"], "check");
    assert_eq!(json["files"].as_array().unwrap().len(), 2);
    assert_eq!(tree_snapshot(temp.path()), before);

    let text = "module main; // é\r\nfn answer()->I64 {return true;}";
    fs::write(&source, text).unwrap();
    let output = run_in(temp.path(), &["check", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["code"], "PTN_TYPE_MISMATCH");
    assert_eq!(error["phase"], "type");
    assert_eq!(error["span"]["path"], "src/main.ptn");
    assert_eq!(error["span"]["start_byte"], text.find("true").unwrap());
    assert_ne!(json["source_revision"], error["source_revision"]);
    let plain = run_in(temp.path(), &["check"]);
    assert!(plain.stdout.is_empty());
    let stderr = String::from_utf8(plain.stderr).unwrap();
    assert!(stderr.contains("src/main.ptn:"));
    assert!(stderr.contains("PTN_TYPE_MISMATCH"));
}

#[test]
fn format_checks_without_writing_and_repeated_formatting_is_stable() {
    let temp = TempDir::new();
    fs::create_dir(temp.path().join("src")).unwrap();
    let source = temp.path().join("src/main.ptn");
    let text = "// é\r\nmodule main;fn answer()->I64{return 42;} // kept";
    fs::write(&source, text).unwrap();
    let before = tree_snapshot(temp.path());
    let output = run_in(temp.path(), &["fmt", "--check", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["code"], "PTN_TOOL_FORMAT_REQUIRED");
    assert_eq!(error["span"]["path"], "src/main.ptn");
    assert_eq!(tree_snapshot(temp.path()), before);
    let output = run_in(temp.path(), &["fmt", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["kind"], "format");
    assert_eq!(json["files"][0]["changed"], true);
    let once = fs::read_to_string(&source).unwrap();
    assert!(once.contains("// kept"));
    assert!(once.ends_with('\n'));
    assert_ne!(once, text);
    let before = tree_snapshot(temp.path());
    let output = run_in(temp.path(), &["fmt", "--check", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(tree_snapshot(temp.path()), before);
    let output = run_in(temp.path(), &["fmt", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["files"][0]["changed"], false);
    assert_eq!(tree_snapshot(temp.path()), before);
}

#[test]
fn formatting_validates_the_entire_selection_before_any_write() {
    let temp = TempDir::new();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("src/a.ptn"),
        "module a;fn run()->I64{return 1;}",
    )
    .unwrap();
    fs::write(temp.path().join("src/z.ptn"), "module z; fn").unwrap();
    let before = tree_snapshot(temp.path());
    let output = run_in(temp.path(), &["fmt", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["phase"], "parse");
    assert_eq!(json["span"]["path"], "src/z.ptn");
    assert_eq!(tree_snapshot(temp.path()), before);
    fs::write(temp.path().join("src/z.ptn"), [255]).unwrap();
    let before = tree_snapshot(temp.path());
    let output = run_in(temp.path(), &["fmt", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["code"], "PTN_LEX_INVALID_UTF8");
    assert!(json.get("span").is_none());
    assert_eq!(tree_snapshot(temp.path()), before);
}

#[test]
fn explicit_selections_empty_sources_and_overlaps_have_clear_results() {
    let temp = TempDir::new();
    let output = run_in(temp.path(), &["check", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["code"], "PTN_TOOL_NO_SOURCES");
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("src/main.ptn"),
        "module main; fn run()->Unit {}",
    )
    .unwrap();
    fs::write(temp.path().join("src/broken.ptn"), "not source").unwrap();
    let output = run_in(temp.path(), &["check", "./src/main.ptn", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let output = run_in(temp.path(), &["fmt", "src", "src/main.ptn", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["code"], "PTN_TOOL_INVALID_ARGUMENTS");
    let output = run_in(temp.path(), &["check", "missing.ptn", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["code"], "PTN_TOOL_IO");
}

#[cfg(unix)]
#[test]
fn formatting_preserves_permissions_and_rejects_source_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = TempDir::new();
    fs::create_dir(temp.path().join("src")).unwrap();
    let path = temp.path().join("src/main.ptn");
    fs::write(&path, "module main;fn run()->Unit{}").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert_eq!(
        run_in(temp.path(), &["fmt", "--json"]).status.code(),
        Some(0)
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    symlink(&path, temp.path().join("src/alias.ptn")).unwrap();
    let before = fs::read(&path).unwrap();
    let output = run_in(temp.path(), &["fmt", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["code"], "PTN_TOOL_INVALID_ARGUMENTS");
    assert_eq!(fs::read(&path).unwrap(), before);
}

fn run_os_in(directory: &Path, args: Vec<OsString>) -> Output {
    Command::new(CLI)
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap()
}

fn assert_project_error(output: Output) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(matches!(
        value["code"].as_str(),
        Some("PTN_CHANGE_INVALID_REQUEST" | "PTN_TOOL_OPERATION")
    ));
}

fn tree_snapshot(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    fn walk(root: &Path, current: &Path, entries: &mut Vec<(PathBuf, Option<Vec<u8>>)>) {
        for item in fs::read_dir(current).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() {
                entries.push((path.strip_prefix(root).unwrap().to_path_buf(), None));
                walk(root, &path, entries);
            } else {
                entries.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    Some(fs::read(path).unwrap()),
                ));
            }
        }
    }
    let mut entries = Vec::new();
    walk(root, root, &mut entries);
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "pactern-cli-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn line_limits_reject_input_and_formatter_expansion_without_writes() {
    let temp = TempDir::new();
    fs::create_dir(temp.path().join("src")).unwrap();
    let path = temp.path().join("src/main.ptn");
    let too_long = format!("module main;\n{}// excess", "// é\r\n".repeat(999));
    fs::write(&path, &too_long).unwrap();
    let output = run_in(temp.path(), &["check", "--json"]);
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(result["code"], "PTN_LEX_LINE_LIMIT");
    assert_eq!(
        result["span"]["start_byte"].as_u64().unwrap() as usize,
        too_long.len() - 9
    );
    let dense = format!("module main; fn main()->Unit{{{}}}", "();".repeat(1000));
    fs::write(&path, &dense).unwrap();
    let before = tree_snapshot(temp.path());
    let output = run_in(temp.path(), &["fmt", "--json"]);
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["code"], "PTN_TOOL_FORMAT_LINE_LIMIT");
    assert_eq!(tree_snapshot(temp.path()), before);
}

#[test]
fn cli_compiles_runs_verifies_and_executes_seeded_properties() {
    let temp = TempDir::new();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::create_dir(temp.path().join("contracts")).unwrap();
    fs::write(
        temp.path().join("src/main.ptn"),
        "module main; pub fn main(x:I64)->I64{return x+1;}",
    )
    .unwrap();
    fs::write(
        temp.path().join("contracts/main.pact.ptn"),
        "module main; pact p for main {requires x>=0 && x<100;ensures result==x+1;}",
    )
    .unwrap();
    for args in [
        vec!["run", "--args", "[\"4\"]", "--json"],
        vec!["build", "--args", "[\"4\"]", "--json"],
        vec!["verify", "--json"],
        vec!["test", "--cases", "3", "--seed", "9", "--json"],
    ] {
        let output = run_in(temp.path(), &args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        if args[0] == "run" {
            assert_eq!(value["outcome"]["value"], "5");
        }
        if args[0] == "build" {
            let guards = value["guard_map"].as_array().unwrap();
            assert_eq!(guards.len(), 2);
            assert!(
                guards
                    .iter()
                    .all(|guard| guard["artifact_hash"] == value["artifact_hash"])
            );
        }
    }
    assert!(
        fs::read(temp.path().join("target/pactern.wasm"))
            .unwrap()
            .starts_with(b"\0asm")
    );
    let output = run_in(temp.path(), &["run", "--args", "[\"-1\"]", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["outcome"]["trap"], "Precondition");
    let output = run_in(
        temp.path(),
        &["run", "--args", "[9007199254740993]", "--json"],
    );
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn managed_cli_repairs_defect_and_rejects_stale_or_unproved_changes() {
    let temp = TempDir::new();
    let project = temp.path().join("candidate");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::create_dir(project.join("contracts")).unwrap();
    fs::write(
        project.join("src/main.ptn"),
        "module main; pub fn main(x:I64)->I64{return x+2;}",
    )
    .unwrap();
    fs::write(
        project.join("contracts/main.pact.ptn"),
        "module main; pact p for main {requires x>=0 && x<100;ensures result==x+1;}",
    )
    .unwrap();
    let store = temp.path().join("store");
    let baseline = temp.path().join("baseline.json");
    let store = store.to_str().unwrap();
    let baseline = baseline.to_str().unwrap();
    let output = run_in(
        &project,
        &["init", "--store", store, "--baseline", baseline, "--json"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let init: Value = serde_json::from_slice(&output.stdout).unwrap();
    let target = init["declarations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "main")
        .unwrap();
    let request = serde_json::json!({"protocol_version":"0.1.0","kind":"change_request","request_id":"repair-1","transaction":"implementation","base_revision":init["source_revision"],"expected_policy_baseline":init["policy_baseline"],"operations":[{"op":"replace_function_body","target":{"symbol_id":target["id"],"expected_hash":target["declaration_hash"]},"body":"{ return x+1; }"}]});
    let change = project.join("change.json");
    fs::write(&change, serde_json::to_vec(&request).unwrap()).unwrap();
    let change = change.to_str().unwrap();
    let before = fs::read(Path::new(store).join("workspace.json")).unwrap();
    let output = run_in(
        &project,
        &[
            "apply",
            change,
            "--dry-run",
            "--store",
            store,
            "--baseline",
            baseline,
            "--json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        fs::read(Path::new(store).join("workspace.json")).unwrap(),
        before
    );
    let output = run_in(
        &project,
        &[
            "apply",
            change,
            "--store",
            store,
            "--baseline",
            baseline,
            "--json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let committed = fs::read(Path::new(store).join("workspace.json")).unwrap();
    assert_ne!(committed, before);
    let output = run_in(
        &project,
        &[
            "run",
            "--store",
            store,
            "--baseline",
            baseline,
            "--args",
            "[\"4\"]",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["outcome"]["value"], "5");
    let mut stale = request.clone();
    stale["request_id"] = Value::String("stale-2".into());
    fs::write(change, serde_json::to_vec(&stale).unwrap()).unwrap();
    let output = run_in(
        &project,
        &[
            "apply",
            change,
            "--store",
            store,
            "--baseline",
            baseline,
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        fs::read(Path::new(store).join("workspace.json")).unwrap(),
        committed
    );
    let output = run_in(
        &project,
        &[
            "inspect",
            "main::main",
            "--store",
            store,
            "--baseline",
            baseline,
            "--json",
        ],
    );
    let inspect: Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut bad = request;
    bad["request_id"] = Value::String("bad-3".into());
    bad["base_revision"] = inspect["source_revision"].clone();
    bad["operations"][0]["target"]["expected_hash"] = inspect["result"]["declaration_hash"].clone();
    bad["operations"][0]["body"] = Value::String("{return x+2;}".into());
    fs::write(change, serde_json::to_vec(&bad).unwrap()).unwrap();
    let output = run_in(
        &project,
        &[
            "apply",
            change,
            "--store",
            store,
            "--baseline",
            baseline,
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["code"], "PTN_POLICY_REQUIRED_PROOF");
    assert_eq!(
        fs::read(Path::new(store).join("workspace.json")).unwrap(),
        committed
    );
}

#[test]
fn editor_cli_reports_diagnostics_and_resolves_declarations() {
    let temp = TempDir::new();
    fs::create_dir(temp.path().join("src")).unwrap();
    let path = temp.path().join("src/main.ptn");
    let text = "module main; fn value()->I64{return 1;} fn main()->I64{return value();}";
    fs::write(&path, text).unwrap();
    let byte = text.rfind("value()").unwrap().to_string();
    let output = run_in(
        temp.path(),
        &[
            "editor",
            "--path",
            "src/main.ptn",
            "--byte",
            &byte,
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["start_byte"], text.find("value()").unwrap());
    fs::write(&path, "module main; fn main()->I64{return false;}").unwrap();
    let output = run_in(temp.path(), &["editor", "--json"]);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"][0]["code"], "PTN_TYPE_MISMATCH");
    assert_eq!(value["result"][0]["range"]["start"]["line"], 0);
}

#[test]
fn variant_arguments_are_exact_and_custom_err_round_trips() {
    let temp = TempDir::new();
    fs::create_dir(temp.path().join("src")).unwrap();
    let source = temp.path().join("src/main.ptn");
    fs::write(
        &source,
        "module main; pub enum E { Err(Unit), Empty } pub fn main(x:E)->E{return x;}",
    )
    .unwrap();
    let output = run_in(
        temp.path(),
        &[
            "run",
            "--args",
            "[{\"tag\":\"Err\",\"value\":null}]",
            "--json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["outcome"]["value"],
        serde_json::json!({"tag":"Err","value":null})
    );
    for args in [
        "[{\"tag\":\"Err\"}]",
        "[{\"tag\":\"Err\",\"error\":null}]",
        "[{\"tag\":\"Empty\",\"value\":null}]",
        "[{\"tag\":\"Err\",\"value\":null,\"extra\":true}]",
        "[{\"tag\":\"Empty\",\"tag\":\"Empty\"}]",
    ] {
        let output = run_in(temp.path(), &["run", "--args", args, "--json"]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    fs::write(
        &source,
        "module main; pub fn main(x:Option<Unit>)->Option<Unit>{return x;}",
    )
    .unwrap();
    for args in [
        "[\"Some\"]",
        "[{\"tag\":\"Some\"}]",
        "[{\"tag\":\"None\",\"value\":null}]",
    ] {
        assert_eq!(
            run_in(temp.path(), &["run", "--args", args, "--json"])
                .status
                .code(),
            Some(2)
        );
    }
    let output = run_in(
        temp.path(),
        &[
            "run",
            "--args",
            "[{\"tag\":\"Some\",\"value\":null}]",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn artifact_output_cannot_alias_its_sidecar_or_overwrite_sources() {
    let temp = TempDir::new();
    fs::create_dir(temp.path().join("src")).unwrap();
    let source = temp.path().join("src/main.ptn");
    let content = "module main; pub fn main()->I64{return 42;}";
    fs::write(&source, content).unwrap();
    for output in ["artifact.c", "artifact.C", "src/main.ptn"] {
        let result = run_in(temp.path(), &["build", "--output", output, "--json"]);
        assert_eq!(
            result.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert_eq!(fs::read_to_string(&source).unwrap(), content);
    }
    assert!(!temp.path().join("artifact.c").exists());
    let result = run_in(temp.path(), &["test", "--cases", "0", "--json"]);
    assert!(!result.status.success());
}

#[test]
fn artifact_output_cannot_overwrite_trusted_baseline_or_managed_store() {
    let temp = TempDir::new();
    let project = temp.path().join("candidate");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("src/main.ptn"),
        "module main; pub fn main()->I64{return 42;}",
    )
    .unwrap();
    let store = temp.path().join("store");
    let baseline = temp.path().join("baseline.json");
    let initialized = run_in(
        &project,
        &[
            "init",
            "--store",
            store.to_str().unwrap(),
            "--baseline",
            baseline.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stdout)
    );
    let before_baseline = fs::read(&baseline).unwrap();
    let before_store = fs::read(store.join("workspace.json")).unwrap();
    for output in [
        &baseline,
        &store.join("workspace.json"),
        &store.join("artifact.wasm"),
        &store.join("uncreated/artifact.wasm"),
    ] {
        let result = run_in(
            &project,
            &[
                "build",
                "--store",
                store.to_str().unwrap(),
                "--baseline",
                baseline.to_str().unwrap(),
                "--output",
                output.to_str().unwrap(),
                "--json",
            ],
        );
        assert_eq!(
            result.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert_eq!(fs::read(&baseline).unwrap(), before_baseline);
        assert_eq!(
            fs::read(store.join("workspace.json")).unwrap(),
            before_store
        );
    }
}
