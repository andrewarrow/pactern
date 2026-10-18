//! Artifact- and policy-bound evidence. Proofs, guards and executed tests remain
//! separate. These records are data, not signatures: a trusted acceptance tool
//! must construct and validate them outside candidate-controlled changes.
use crate::{
    pacts::TestReport,
    verification::{Report, Status},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub required_proof_functions: BTreeSet<usize>,
    pub required_guards: BTreeSet<String>,
    pub required_tests: BTreeSet<String>,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub program_hash: String,
    pub source_hashes: BTreeMap<String, String>,
    pub dependency_hashes: BTreeMap<String, String>,
    pub compiler_version: String,
    pub compiler_hash: String,
    pub solver_version: String,
    pub solver_hash: String,
    pub target: String,
    pub artifact_hash: String,
    pub policy_hash: String,
    pub assumptions: Vec<String>,
}
pub struct BindingInputs<'a> {
    pub program: &'a crate::core::Program,
    pub sources: &'a BTreeMap<String, Vec<u8>>,
    pub dependencies: &'a BTreeMap<String, Vec<u8>>,
    pub compiler_bytes: &'a [u8],
    pub solver_version: &'a str,
    pub solver_bytes: &'a [u8],
    pub target: &'a str,
    pub artifact: &'a [u8],
    pub policy: &'a Policy,
    pub assumptions: Vec<String>,
}
pub fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
impl Binding {
    pub fn new(input: BindingInputs<'_>) -> Self {
        Self {
            program_hash: crate::verification::program_fingerprint(input.program),
            source_hashes: input
                .sources
                .iter()
                .map(|(p, b)| (p.clone(), hash(b)))
                .collect(),
            dependency_hashes: input
                .dependencies
                .iter()
                .map(|(p, b)| (p.clone(), hash(b)))
                .collect(),
            compiler_version: crate::TOOL_VERSION.into(),
            compiler_hash: hash(input.compiler_bytes),
            solver_version: input.solver_version.into(),
            solver_hash: hash(input.solver_bytes),
            target: input.target.into(),
            artifact_hash: hash(input.artifact),
            policy_hash: policy_hash(input.policy),
            assumptions: input.assumptions,
        }
    }
    pub fn revision(&self) -> String {
        hash(&serde_json::to_vec(self).expect("Binding contains only serializable values"))
    }
}
fn policy_hash(policy: &Policy) -> String {
    hash(&serde_json::to_vec(policy).expect("Policy contains only serializable values"))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardEvidence {
    pub obligation_id: String,
    /// Must equal the enclosing artifact. Supplied by the compiler's guard map.
    pub artifact_hash: String,
    pub phase: String,
    pub function: usize,
    pub clause: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub schema_version: String,
    pub binding: Binding,
    pub proofs: Vec<Report>,
    pub guards: Vec<GuardEvidence>,
    pub tests: BTreeMap<String, TestReport>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Acceptance {
    pub accepted: bool,
    pub reasons: Vec<String>,
}
impl Evidence {
    /// Exact comparison invalidates cached evidence on every bound input change.
    pub fn reusable_for(&self, expected: &Binding) -> bool {
        &self.binding == expected
    }
    pub fn evaluate(&self, expected: &Binding, policy: &Policy) -> Acceptance {
        let mut reasons = Vec::new();
        if self.schema_version != "0.1.0" {
            reasons.push("unsupported evidence schema".into());
        }
        if !self.reusable_for(expected) {
            reasons.push("source, dependency, toolchain, artifact, assumptions, target, or policy binding changed".into());
        }
        if self.binding.policy_hash != policy_hash(policy) {
            reasons.push("policy hash mismatch".into());
        }
        let mut proof_targets = BTreeSet::new();
        for proof in &self.proofs {
            if proof.program_hash != self.binding.program_hash {
                reasons.push(format!(
                    "proof for function {} belongs to another checked program",
                    proof.function
                ));
            }
            if !proof_targets.insert(proof.function) {
                reasons.push(format!("duplicate proof for function {}", proof.function));
            }
            if proof
                .assumptions
                .iter()
                .any(|assumption| !self.binding.assumptions.contains(assumption))
            {
                reasons.push(format!(
                    "proof for function {} has assumptions absent from the evidence binding",
                    proof.function
                ));
            }
            if proof.solver != self.binding.solver_version {
                reasons.push(format!(
                    "solver version mismatch for function {}",
                    proof.function
                ));
            }
            if [
                &proof.precondition_consistency,
                &proof.postcondition,
                &proof.absence_of_traps,
                &proof.termination,
            ]
            .iter()
            .any(|c| c.status == Status::Refuted)
            {
                reasons.push(format!(
                    "function {} has a refuted obligation",
                    proof.function
                ));
            }
        }
        for function in &policy.required_proof_functions {
            match self.proofs.iter().find(|p| p.function == *function) {
                Some(proof)
                    if proof.exploration_bound.is_none()
                        && [
                            &proof.precondition_consistency,
                            &proof.postcondition,
                            &proof.absence_of_traps,
                            &proof.termination,
                        ]
                        .iter()
                        .all(|c| c.status == Status::Proved) => {}
                _ => reasons.push(format!(
                    "function {function} lacks all required unbounded proofs"
                )),
            }
        }
        let mut guard_ids = BTreeSet::new();
        for guard in &self.guards {
            if !guard_ids.insert(&guard.obligation_id) {
                reasons.push(format!("duplicate runtime guard {}", guard.obligation_id));
            }
            if guard.artifact_hash != self.binding.artifact_hash {
                reasons.push(format!(
                    "guard {} belongs to another artifact",
                    guard.obligation_id
                ));
            }
        }
        for required in &policy.required_guards {
            if !guard_ids.contains(required) {
                reasons.push(format!("missing runtime guard {required}"));
            }
        }
        for (name, test) in &self.tests {
            if test.program_hash != self.binding.program_hash {
                reasons.push(format!("test {name} belongs to another checked program"));
            }
        }
        for required in &policy.required_tests {
            if !self.tests.get(required).is_some_and(|test| {
                test.passed() && test.requested > 0 && test.compiled_engine.is_some()
            }) {
                reasons.push(format!(
                    "required executed test {required} failed, is missing, or was skipped"
                ));
            }
        }
        Acceptance {
            accepted: reasons.is_empty(),
            reasons,
        }
    }
}
