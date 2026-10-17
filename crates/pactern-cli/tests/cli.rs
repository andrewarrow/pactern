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
fn unsupported_commands_accept_unicode_paths_without_touching_project() {
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
        vec!["--json", "check"],
        vec!["fmt", "--check", "--json"],
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
        assert_eq!(value["code"], "PTN_TOOL_NOT_IMPLEMENTED", "args: {args:?}");
    }

    for path in [&unicode_change, &missing_change] {
        let mut impact_args: Vec<OsString> =
            vec!["--json".into(), "impact".into(), "--change".into()];
        impact_args.push(path.as_os_str().to_owned());
        assert_not_implemented(run_os_in(&project, impact_args));

        for dry_run in [false, true] {
            let mut apply_args: Vec<OsString> = vec!["--json".into(), "apply".into()];
            apply_args.push(path.as_os_str().to_owned());
            if dry_run {
                apply_args.push("--dry-run".into());
            }
            assert_not_implemented(run_os_in(&project, apply_args));
        }
    }
    assert_eq!(tree_snapshot(&project), before);
}

fn run_os_in(directory: &Path, args: Vec<OsString>) -> Output {
    Command::new(CLI)
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap()
}

fn assert_not_implemented(output: Output) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["code"], "PTN_TOOL_NOT_IMPLEMENTED");
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
        Self(path)
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
