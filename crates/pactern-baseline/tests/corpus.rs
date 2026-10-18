use pactern_baseline::{Corpus, Outcome, Response, Tasks, compare, read_json};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn corpus() -> Corpus {
    read_json(&root().join("fixtures/conformance/manifest.json")).unwrap()
}
fn tasks() -> Tasks {
    read_json(&root().join("baselines/tasks.json")).unwrap()
}

#[test]
fn recorded_corpus_has_readable_sources_and_valid_expectations() {
    let corpus = corpus();
    corpus
        .validate(&root().join("fixtures/conformance"))
        .unwrap();
    for kind in ["return", "trap", "diagnostic", "verification"] {
        assert!(
            corpus
                .cases
                .iter()
                .any(|case| serde_json::to_value(&case.expected).unwrap()["kind"] == kind)
        );
    }
    let (requests, expected) = tasks().requests(&corpus).unwrap();
    assert!(!requests.is_empty());
    assert_eq!(requests.len(), expected.len());
}

#[test]
fn corpus_rejects_duplicate_ids_and_escaping_paths() {
    let mut corpus = corpus();
    corpus.cases[1].id = corpus.cases[0].id.clone();
    assert!(
        corpus
            .validate(&root().join("fixtures/conformance"))
            .is_err()
    );
    for path in [
        "../README.md",
        "/tmp/input.ptn",
        "src//quota.ptn",
        "src/./quota.ptn",
        "src\\quota.ptn",
    ] {
        let mut corpus = self::corpus();
        corpus.cases[0].source = path.to_owned();
        assert!(
            corpus
                .validate(&root().join("fixtures/conformance"))
                .is_err(),
            "{path}"
        );
    }
}

#[test]
fn corpus_rejects_contradictory_or_missing_runtime_pact_states() {
    let mut corpus = corpus();
    corpus.cases[0].pacts.as_mut().unwrap().postcondition = pactern_baseline::PactState::Fail;
    assert!(
        corpus
            .validate(&root().join("fixtures/conformance"))
            .is_err()
    );
    corpus.cases[0].pacts = None;
    assert!(
        corpus
            .validate(&root().join("fixtures/conformance"))
            .is_err()
    );
}

#[test]
fn tasks_reject_missing_cases_wrong_source_and_inexact_i64_inputs() {
    let mut tasks = tasks();
    tasks.tasks[0].case_ids.push("missing-case".to_owned());
    assert!(tasks.requests(&corpus()).is_err());
    let mut tasks = self::tasks();
    tasks.tasks[0].source = "src/summarize.ptn".to_owned();
    assert!(tasks.requests(&corpus()).is_err());
    for invalid in [
        json!(3),
        json!("03"),
        json!("-0"),
        json!("9223372036854775808"),
    ] {
        let mut corpus = corpus();
        corpus.cases[0].args[0] = invalid;
        assert!(self::tasks().requests(&corpus).is_err());
    }
}

#[test]
fn comparison_rejects_incorrect_values_pacts_and_response_sets() {
    let (_, expected) = tasks().requests(&corpus()).unwrap();
    let output = serde_json::to_vec(&expected).unwrap();
    compare(&expected, &output).unwrap();
    let mut wrong = expected.clone();
    wrong[0].outcome = Outcome::Return {
        value: json!({"tag": "Ok", "value": "999"}),
    };
    assert!(compare(&expected, &serde_json::to_vec(&wrong).unwrap()).is_err());
    let mut wrong = expected.clone();
    wrong[0].pacts = None;
    assert!(compare(&expected, &serde_json::to_vec(&wrong).unwrap()).is_err());
    let mut wrong = expected.clone();
    wrong.swap(0, 1);
    assert!(compare(&expected, &serde_json::to_vec(&wrong).unwrap()).is_err());
    assert!(compare(&expected, &serde_json::to_vec(&expected[1..]).unwrap()).is_err());
    let mut wrong: Value = serde_json::from_slice(&output).unwrap();
    wrong[0]["unrecognized"] = json!(true);
    assert!(compare(&expected, &serde_json::to_vec(&wrong).unwrap()).is_err());
}

#[test]
fn conventional_rust_program_matches_every_selected_case() {
    let (requests, expected) = tasks().requests(&corpus()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_baseline-rust"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&requests).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual: Vec<Response> = compare(&expected, &output.stdout).unwrap();
    assert_eq!(actual.len(), requests.len());
}
