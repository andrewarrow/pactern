//! External baseline enforcement shared by semantic edits and direct-text CI checks.
//!
//! Load this policy and approvals from trusted storage outside the candidate. This
//! module does not treat a candidate-supplied manifest or approval as authority.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::semantic::{SemanticError, SemanticIndex, digest};

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HashRecord {
    pub category: String,
    pub key: String,
    pub digest: String,
}

/// Protocol H framing. Digests are validated and duplicate keys are rejected.
pub fn inventory_hash(tag: &str, records: &[HashRecord]) -> Result<String, SemanticError> {
    let mut ordered: Vec<_> = records.iter().collect();
    ordered.sort_by(|a, b| (&a.category, &a.key).cmp(&(&b.category, &b.key)));
    let mut hash = Sha256::new();
    hash.update(format!("pactern/0.1.0/{tag}\n"));
    let mut previous = None;
    for record in ordered {
        if previous == Some((&record.category, &record.key)) || !valid_digest(&record.digest) {
            return Err(SemanticError::new(
                "PTN_CHANGE_INVALID_REQUEST",
                "duplicate inventory key or invalid content digest",
            ));
        }
        previous = Some((&record.category, &record.key));
        for text in [&record.category, &record.key] {
            hash.update((text.len() as u64).to_be_bytes());
            hash.update(text);
        }
        for pair in record.digest.as_bytes()[7..].as_chunks::<2>().0 {
            let nibble = |b: u8| {
                if b.is_ascii_digit() {
                    b - b'0'
                } else {
                    b - b'a' + 10
                }
            };
            hash.update([nibble(pair[0]) * 16 + nibble(pair[1])]);
        }
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}

pub fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileDiff {
    pub path: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

pub fn exact_diff(
    old: &BTreeMap<String, Vec<u8>>,
    new: &BTreeMap<String, Vec<u8>>,
) -> Vec<FileDiff> {
    let paths: BTreeSet<_> = old.keys().chain(new.keys()).collect();
    paths
        .into_iter()
        .filter_map(|path| {
            let before = old.get(path).map(digest);
            let after = new.get(path).map(digest);
            (before != after).then(|| FileDiff {
                path: path.clone(),
                old: before,
                new: after,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedBaseline {
    protected_files: BTreeMap<String, String>,
    protected_declarations: BTreeMap<String, String>,
    protected_dependencies: BTreeMap<String, BTreeSet<String>>,
    /// Accepted function signatures include declared effect bounds. Names are
    /// excluded so an authorized semantic rename can retain its interface ID.
    public_signatures: BTreeMap<String, String>,
    external_records: Vec<HashRecord>,
    required_checks: BTreeSet<String>,
    pub allow_public_rename: bool,
}

impl TrustedBaseline {
    /// Called only by the external policy owner. Pacts and their helper closure are
    /// protected automatically. A pact's target implementation remains editable.
    pub fn capture(
        files: &BTreeMap<String, Vec<u8>>,
        index: &SemanticIndex,
        protected_paths: &[String],
        protected_ids: &[String],
        external_records: Vec<HashRecord>,
    ) -> Result<Self, SemanticError> {
        let mut protected_files = BTreeMap::new();
        for path in protected_paths {
            let bytes = files.get(path).ok_or_else(|| {
                SemanticError::new(
                    "PTN_POLICY_INVALID_BASELINE",
                    format!("missing protected file {path}"),
                )
            })?;
            protected_files.insert(path.clone(), digest(bytes));
        }
        let mut protected: BTreeSet<_> = protected_ids.iter().cloned().collect();
        protected.extend(
            index
                .declarations
                .iter()
                .filter(|d| d.kind == "pact")
                .map(|d| d.id.clone()),
        );
        loop {
            let before = protected.len();
            for declaration in &index.declarations {
                if !protected.contains(&declaration.id) {
                    continue;
                }
                let header_end = if declaration.kind == "pact" {
                    crate::semantic::pact_body_start(declaration)
                        .unwrap_or(declaration.range.end_byte)
                } else {
                    0
                };
                for reference in index
                    .references
                    .iter()
                    .filter(|r| r.owner == declaration.id)
                {
                    // A pact target in its header is an interface binding, not a
                    // helper whose implementation becomes a protected oracle.
                    if declaration.kind != "pact" || reference.range.start_byte > header_end {
                        protected.insert(reference.target.clone());
                    }
                }
            }
            if before == protected.len() {
                break;
            }
        }
        let mut protected_declarations = BTreeMap::new();
        let mut protected_dependencies = BTreeMap::new();
        for id in protected {
            let declaration = index.find(&id)?;
            protected_declarations.insert(id.clone(), declaration.declaration_hash.clone());
            protected_dependencies.insert(id, declaration.dependencies.clone());
        }
        let public_signatures = index
            .declarations
            .iter()
            .filter(|d| d.kind == "function")
            .map(|d| (d.id.clone(), interface_signature(d)))
            .collect();
        let baseline = Self {
            protected_files,
            protected_declarations,
            protected_dependencies,
            public_signatures,
            external_records,
            required_checks: BTreeSet::from(["frontend".into()]),
            allow_public_rename: false,
        };
        baseline.digest()?;
        Ok(baseline)
    }

    pub fn require_check(&mut self, name: impl Into<String>) {
        self.required_checks.insert(name.into());
    }
    pub fn required_checks(&self) -> &BTreeSet<String> {
        &self.required_checks
    }

    pub fn digest(&self) -> Result<String, SemanticError> {
        let mut records = self.external_records.clone();
        if records.iter().any(|r| {
            !matches!(
                r.category.as_str(),
                "file" | "dependency" | "configuration" | "tool"
            )
        }) {
            return Err(SemanticError::new(
                "PTN_POLICY_INVALID_BASELINE",
                "policy baseline permits only file, dependency, configuration, and tool records",
            ));
        }
        for (path, hash) in &self.protected_files {
            records.push(HashRecord {
                category: "file".into(),
                key: format!("protected/{path}"),
                digest: hash.clone(),
            });
        }
        for (id, hash) in &self.protected_declarations {
            records.push(HashRecord {
                category: "configuration".into(),
                key: format!("protected-identity/{id}"),
                digest: hash.clone(),
            });
        }
        records.push(HashRecord {
            category: "configuration".into(),
            key: "acceptance-policy".into(),
            digest: digest(
                serde_json::to_vec(&(
                    &self.protected_dependencies,
                    &self.public_signatures,
                    &self.required_checks,
                    self.allow_public_rename,
                ))
                .map_err(json_error)?,
            ),
        });
        inventory_hash("policy", &records)
    }

    /// Invoke this on direct-text candidates in CI as well as local transactions.
    /// This gate checks protected content/bindings; evidence checks are additional.
    pub fn check(
        &self,
        files: &BTreeMap<String, Vec<u8>>,
        index: &SemanticIndex,
    ) -> Result<(), SemanticError> {
        for (path, expected) in &self.protected_files {
            if files.get(path).map(digest).as_ref() != Some(expected) {
                return Err(protected(format!(
                    "protected file {path} changed or disappeared"
                )));
            }
        }
        for (id, expected) in &self.protected_declarations {
            let declaration = index
                .find(id)
                .map_err(|_| protected(format!("protected identity {id} disappeared")))?;
            if &declaration.declaration_hash != expected
                || self.protected_dependencies.get(id) != Some(&declaration.dependencies)
            {
                return Err(protected(format!(
                    "protected declaration or resolved bindings of {id} changed"
                )));
            }
        }
        for (id, signature) in &self.public_signatures {
            let declaration = index
                .find(id)
                .map_err(|_| protected(format!("accepted interface {id} disappeared")))?;
            if &interface_signature(declaration) != signature {
                return Err(protected(format!(
                    "accepted interface/effect bound {id} changed"
                )));
            }
        }
        // Without complete dependency data a changed helper could alter protected
        // meaning invisibly. Acceptance requires resolving that uncertainty.
        if !index.complete && !self.protected_declarations.is_empty() {
            return Err(protected("protected dependency closure is incomplete"));
        }
        Ok(())
    }
}

pub(crate) fn json_error(error: serde_json::Error) -> SemanticError {
    SemanticError::new("PTN_CHANGE_INVALID_REQUEST", error.to_string())
}
fn protected(message: impl Into<String>) -> SemanticError {
    SemanticError::new("PTN_CHANGE_PROTECTED_TARGET", message)
}

fn interface_signature(declaration: &crate::semantic::Declaration) -> String {
    let mut signature = declaration.signature.clone();
    signature.replace_range(
        declaration.name_range.start_byte - declaration.range.start_byte
            ..declaration.name_range.end_byte - declaration.range.start_byte,
        "<name>",
    );
    signature
}

/// An external host capability, never a field accepted in a change request.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    base_revision: String,
    candidate_revision: String,
    policy_baseline: String,
    diff: Vec<FileDiff>,
}
impl Approval {
    /// Only a trusted approval service may call this or deserialize its output.
    pub fn authorize(
        base_revision: String,
        candidate_revision: String,
        policy_baseline: String,
        diff: Vec<FileDiff>,
    ) -> Self {
        Self {
            base_revision,
            candidate_revision,
            policy_baseline,
            diff,
        }
    }
    pub fn matches(&self, base: &str, candidate: &str, policy: &str, diff: &[FileDiff]) -> bool {
        self.base_revision == base
            && self.candidate_revision == candidate
            && self.policy_baseline == policy
            && self.diff == diff
    }
}
