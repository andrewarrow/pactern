//! Recorded conformance expectations and conventional-language baseline tasks.
//! This validates corpus metadata; it does not parse or execute Pactern.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Component, Path};

pub type Error = Box<dyn std::error::Error>;
pub const FORMAT_VERSION: &str = "0.1.0";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corpus {
    pub format_version: String,
    pub language_version: String,
    pub cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub source: String,
    #[serde(default)]
    pub contracts: Vec<String>,
    pub entry: Option<String>,
    #[serde(default)]
    pub args: Vec<Value>,
    pub expected: Outcome,
    pub pacts: Option<Pacts>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Return {
        value: Value,
    },
    Trap {
        category: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cause: Option<String>,
    },
    Diagnostic {
        phase: String,
        code: String,
    },
    Verification {
        claim: String,
        status: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Pacts {
    pub precondition: PactState,
    pub postcondition: PactState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PactState {
    Pass,
    Fail,
    NotReached,
    Trap,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tasks {
    pub format_version: String,
    pub tasks: Vec<Task>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub description: String,
    pub source: String,
    pub entry: String,
    pub case_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Request {
    pub id: String,
    pub task: String,
    pub args: Vec<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub id: String,
    pub outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pacts: Option<Pacts>,
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| format!("{}: {error}", path.display()).into())
}

/// Require portable, relative, regular UTF-8 fixture inputs, including no symlinks.
pub fn source_text(root: &Path, relative: &str) -> Result<String, Error> {
    if relative.is_empty()
        || relative.contains(['\\', ':'])
        || relative.chars().any(char::is_control)
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("invalid fixture path: {relative}").into());
    }
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(part) = component else {
            return Err("fixture path must be relative".into());
        };
        path.push(part);
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(format!("fixture symlink: {relative}").into());
        }
    }
    if !path.is_file() {
        return Err(format!("fixture is not a regular file: {relative}").into());
    }
    let source = fs::read_to_string(path)?;
    if source.trim().is_empty() {
        return Err(format!("empty fixture: {relative}").into());
    }
    Ok(source)
}

impl Corpus {
    pub fn validate(&self, root: &Path) -> Result<(), Error> {
        if self.format_version != FORMAT_VERSION || self.language_version != "0.1.0" {
            return Err("unsupported corpus version".into());
        }
        if self.cases.is_empty() {
            return Err("empty corpus".into());
        }
        let mut ids = BTreeSet::new();
        for case in &self.cases {
            if case.id.is_empty() || !ids.insert(&case.id) {
                return Err(format!("empty or duplicate case ID: {}", case.id).into());
            }
            source_text(root, &case.source)?;
            for contract in &case.contracts {
                source_text(root, contract)?;
            }
            match &case.expected {
                Outcome::Diagnostic { phase, code } => {
                    if ![
                        "lex", "parse", "resolve", "type", "effect", "contract", "verify",
                    ]
                    .contains(&phase.as_str())
                        || !code.starts_with("PTN_")
                    {
                        return Err(format!("invalid diagnostic expectation: {}", case.id).into());
                    }
                }
                Outcome::Trap { category, cause } => {
                    if ![
                        "integer_overflow",
                        "division_by_zero",
                        "bounds",
                        "precondition",
                        "postcondition",
                        "contract_evaluation",
                    ]
                    .contains(&category.as_str())
                        || (category == "contract_evaluation") != cause.is_some()
                    {
                        return Err(format!("invalid trap expectation: {}", case.id).into());
                    }
                }
                Outcome::Verification { claim, status } => {
                    if ![
                        "precondition_consistency",
                        "postcondition",
                        "absence_of_traps",
                        "termination",
                    ]
                    .contains(&claim.as_str())
                        || !["proved", "refuted", "unknown", "unsupported"]
                            .contains(&status.as_str())
                    {
                        return Err(format!("invalid verification expectation: {}", case.id).into());
                    }
                }
                Outcome::Return { .. } => {}
            }
            if !matches!(case.expected, Outcome::Diagnostic { .. })
                && case.entry.as_ref().is_none_or(String::is_empty)
            {
                return Err(format!("missing entry: {}", case.id).into());
            }
            if case.pacts.is_some() && case.contracts.is_empty() {
                return Err(format!("pact states without contracts: {}", case.id).into());
            }
            if let Some(pacts) = &case.pacts {
                use PactState::{Fail, NotReached, Pass, Trap};
                let states = (&pacts.precondition, &pacts.postcondition);
                let valid = match &case.expected {
                    Outcome::Return { .. } => states == (&Pass, &Pass),
                    Outcome::Trap { category, .. } => match category.as_str() {
                        "precondition" => states == (&Fail, &NotReached),
                        "postcondition" => states == (&Pass, &Fail),
                        "contract_evaluation" => {
                            states == (&Trap, &NotReached) || states == (&Pass, &Trap)
                        }
                        _ => states == (&Pass, &NotReached),
                    },
                    _ => false,
                };
                if !valid {
                    return Err(format!("pact states contradict outcome: {}", case.id).into());
                }
            } else if !case.contracts.is_empty()
                && matches!(case.expected, Outcome::Return { .. } | Outcome::Trap { .. })
            {
                return Err(format!("missing runtime pact states: {}", case.id).into());
            }
        }
        Ok(())
    }
}

impl Tasks {
    pub fn requests(&self, corpus: &Corpus) -> Result<(Vec<Request>, Vec<Response>), Error> {
        if self.format_version != FORMAT_VERSION || self.tasks.is_empty() {
            return Err("unsupported or empty tasks".into());
        }
        let mut task_ids = BTreeSet::new();
        let mut selected = BTreeSet::new();
        let mut requests = Vec::new();
        let mut expected = Vec::new();
        for task in &self.tasks {
            if !["quota", "pair_totals", "summarize"].contains(&task.id.as_str())
                || !task_ids.insert(&task.id)
                || task.description.is_empty()
                || task.case_ids.is_empty()
            {
                return Err(format!("invalid task: {}", task.id).into());
            }
            for id in &task.case_ids {
                let case = corpus
                    .cases
                    .iter()
                    .find(|case| &case.id == id)
                    .ok_or_else(|| format!("unknown case: {id}"))?;
                if !selected.insert(id)
                    || case.source != task.source
                    || case.entry.as_ref() != Some(&task.entry)
                    || !matches!(case.expected, Outcome::Return { .. } | Outcome::Trap { .. })
                {
                    return Err(format!("incompatible baseline case: {id}").into());
                }
                validate_arguments(&task.id, &case.args)?;
                requests.push(Request {
                    id: id.clone(),
                    task: task.id.clone(),
                    args: case.args.clone(),
                });
                expected.push(Response {
                    id: id.clone(),
                    outcome: case.expected.clone(),
                    pacts: case.pacts.clone(),
                });
            }
        }
        Ok((requests, expected))
    }
}

fn validate_arguments(task: &str, args: &[Value]) -> Result<(), Error> {
    let integer = |value: &Value| {
        value
            .as_str()
            .and_then(|s| s.parse::<i64>().ok().map(|n| n.to_string() == s))
            .unwrap_or(false)
    };
    let valid = match (task, args) {
        ("quota", [available, amount]) => integer(available) && integer(amount),
        ("pair_totals", [values]) => values
            .as_array()
            .is_some_and(|values| values.iter().all(integer)),
        ("summarize", [label, used, reserved]) => {
            label.is_string() && integer(used) && integer(reserved)
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!("invalid {task} arguments: {args:?}").into())
    }
}

/// Compare the entire ordered response set: missing, extra, reordered, or
/// duplicated results and wrong pact states all fail the run.
pub fn compare(expected: &[Response], output: &[u8]) -> Result<Vec<Response>, Error> {
    let actual: Vec<Response> = serde_json::from_slice(output)?;
    if actual.len() != expected.len() {
        return Err(format!(
            "expected {} responses, received {}",
            expected.len(),
            actual.len()
        )
        .into());
    }
    for (expected, actual) in expected.iter().zip(&actual) {
        if expected != actual {
            return Err(format!(
                "baseline mismatch for {}: expected {expected:?}, received {actual:?}",
                expected.id
            )
            .into());
        }
    }
    Ok(actual)
}
