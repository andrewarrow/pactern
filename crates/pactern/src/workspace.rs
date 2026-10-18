//! Revision-checked semantic transactions over a managed snapshot.
//!
//! A workspace is one owned value, never a sequence of live source overwrites.
//! Memory publication holds one mutex; managed disk publication atomically renames
//! one complete snapshot while holding the store lock. Ordinary editor-owned
//! directories are deliberately not an atomic commit target.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::acceptance::{
    Approval, FileDiff, HashRecord, TrustedBaseline, exact_diff, inventory_hash, json_error,
    valid_digest,
};
use crate::frontend::{SourceInput, SourceSnapshot, valid_path};
use crate::semantic::{
    IdentityLedger, SemanticError, SemanticIndex, SourceRange, digest, valid_id,
};
use crate::syntax::{FileKind, SourceFile, TokenKind, lex};

pub const LEDGER_PATH: &str = ".pactern/identities.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SymbolTarget {
    pub symbol_id: String,
    pub expected_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PactTarget {
    pub pact_id: String,
    pub expected_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    ReplaceFunctionBody {
        target: SymbolTarget,
        body: String,
    },
    RenameSymbol {
        target: SymbolTarget,
        new_name: String,
    },
    ReplacePact {
        target: PactTarget,
        declaration: String,
    },
}
impl Operation {
    pub fn target_id(&self) -> &str {
        match self {
            Self::ReplaceFunctionBody { target, .. } | Self::RenameSymbol { target, .. } => {
                &target.symbol_id
            }
            Self::ReplacePact { target, .. } => &target.pact_id,
        }
    }
    fn expected_hash(&self) -> &str {
        match self {
            Self::ReplaceFunctionBody { target, .. } | Self::RenameSymbol { target, .. } => {
                &target.expected_hash
            }
            Self::ReplacePact { target, .. } => &target.expected_hash,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Transaction {
    Implementation,
    Specification,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeRequest {
    pub protocol_version: String,
    pub kind: String,
    pub request_id: String,
    pub transaction: Transaction,
    pub base_revision: String,
    pub expected_policy_baseline: String,
    pub operations: Vec<Operation>,
}
impl ChangeRequest {
    /// Strict recursive duplicate-key decoding precedes typed schema validation.
    pub fn decode(bytes: &[u8]) -> Result<Self, SemanticError> {
        let request: Self = decode_strict(bytes)?;
        request.validate()?;
        Ok(request)
    }
    pub fn validate(&self) -> Result<(), SemanticError> {
        let valid_request_id = !self.request_id.is_empty()
            && self.request_id.len() <= 128
            && self.request_id.as_bytes()[0].is_ascii_alphanumeric()
            && self
                .request_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
        if self.protocol_version != crate::PROTOCOL_VERSION
            || self.kind != "change_request"
            || !valid_request_id
            || !valid_digest(&self.base_revision)
            || !valid_digest(&self.expected_policy_baseline)
            || self.operations.is_empty()
        {
            return Err(invalid(
                "invalid request envelope, version, identifier, digest, or empty operations",
            ));
        }
        let mut targets = BTreeSet::new();
        for operation in &self.operations {
            let prefix = if matches!(operation, Operation::ReplacePact { .. }) {
                "pact_"
            } else {
                "sym_"
            };
            if !valid_id(operation.target_id(), prefix)
                || !valid_digest(operation.expected_hash())
                || !targets.insert(operation.target_id())
            {
                return Err(invalid("invalid or repeated operation target"));
            }
            match operation {
                Operation::ReplacePact { declaration, .. }
                    if self.transaction != Transaction::Specification
                        || declaration.trim().is_empty() =>
                {
                    return Err(invalid(
                        "pact replacement requires a specification transaction and nonempty declaration",
                    ));
                }
                Operation::ReplaceFunctionBody { body, .. } if body.trim().is_empty() => {
                    return Err(invalid("empty function body"));
                }
                Operation::RenameSymbol { new_name, .. } => {
                    let tokens = lex(&SourceFile::new("rename", new_name))
                        .map_err(|_| invalid("invalid rename identifier"))?;
                    if tokens.len() != 2
                        || tokens[0].kind != TokenKind::Identifier
                        || tokens[0].text != *new_name
                    {
                        return Err(invalid("rename requires one nonreserved identifier"));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceData {
    pub files: BTreeMap<String, Vec<u8>>,
    pub ledger: IdentityLedger,
    /// Trusted dependency content roots, semantic configuration, and tool digests.
    pub external_inputs: Vec<HashRecord>,
    #[serde(default)]
    outcomes: BTreeMap<String, RecordedOutcome>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedOutcome {
    payload: String,
    receipt: CommitReceipt,
}

#[derive(Clone, Debug)]
pub struct WorkspaceSnapshot {
    pub revision: String,
    pub policy_baseline: String,
    pub files: BTreeMap<String, Vec<u8>>,
    pub index: SemanticIndex,
}

impl WorkspaceData {
    pub fn new(
        files: BTreeMap<String, Vec<u8>>,
        external_inputs: Vec<HashRecord>,
    ) -> Result<Self, SemanticError> {
        let snapshot = source_snapshot(&files)?;
        let index = SemanticIndex::build(&snapshot, &IdentityLedger::default())?;
        Ok(Self {
            files,
            ledger: index.ledger,
            external_inputs,
            outcomes: BTreeMap::new(),
        })
    }
    pub fn snapshot(&self, policy: &TrustedBaseline) -> Result<WorkspaceSnapshot, SemanticError> {
        let source = source_snapshot(&self.files)?;
        let mut index = SemanticIndex::build(&source, &self.ledger)?;
        if index.ledger != self.ledger {
            return Err(invalid(
                "source and identity ledger are not reconciled; import the candidate explicitly before inspection",
            ));
        }
        let policy_baseline = policy.digest()?;
        let mut records = self.external_inputs.clone();
        if records
            .iter()
            .any(|r| !matches!(r.category.as_str(), "dependency" | "configuration" | "tool"))
        {
            return Err(invalid(
                "external inventory categories must be dependency, configuration, or tool",
            ));
        }
        let files = self.inventory()?;
        for (path, bytes) in &files {
            records.push(HashRecord {
                category: "file".into(),
                key: path.clone(),
                digest: digest(bytes),
            });
        }
        records.push(HashRecord {
            category: "policy".into(),
            key: "baseline".into(),
            digest: policy_baseline.clone(),
        });
        records.push(HashRecord {
            category: "configuration".into(),
            key: "managed-snapshot-format".into(),
            digest: digest("pactern/0.1.0/managed-snapshot/1"),
        });
        let revision = inventory_hash("workspace", &records)?;
        index.source_revision = revision.clone();
        Ok(WorkspaceSnapshot {
            revision,
            policy_baseline,
            files,
            index,
        })
    }
    fn inventory(&self) -> Result<BTreeMap<String, Vec<u8>>, SemanticError> {
        if self
            .files
            .keys()
            .any(|path| path.eq_ignore_ascii_case(LEDGER_PATH))
        {
            return Err(invalid("identity ledger has a reserved inventory path"));
        }
        let mut files = self.files.clone();
        files.insert(
            LEDGER_PATH.into(),
            serde_json::to_vec(&self.ledger).map_err(json_error)?,
        );
        Ok(files)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitReceipt {
    pub request_id: String,
    pub base_revision: String,
    pub candidate_revision: String,
    pub policy_baseline: String,
    pub diff: Vec<FileDiff>,
    pub performed_checks: BTreeSet<String>,
    pub baseline_advancement_pending: bool,
}

#[derive(Clone, Debug)]
pub struct StagedChange {
    request: ChangeRequest,
    payload: String,
    candidate: WorkspaceData,
    receipt: CommitReceipt,
}
impl StagedChange {
    pub fn preview(&self) -> &CommitReceipt {
        &self.receipt
    }
    pub fn candidate(&self) -> &WorkspaceData {
        &self.candidate
    }
}

#[derive(Clone)]
pub struct Workspace {
    state: Arc<Mutex<(WorkspaceData, TrustedBaseline)>>,
}
impl Workspace {
    pub fn new(data: WorkspaceData, policy: TrustedBaseline) -> Result<Self, SemanticError> {
        data.snapshot(&policy)?;
        Ok(Self {
            state: Arc::new(Mutex::new((data, policy))),
        })
    }
    pub fn snapshot(&self) -> Result<WorkspaceSnapshot, SemanticError> {
        let state = self
            .state
            .lock()
            .map_err(|_| invalid("workspace lock poisoned"))?;
        state.0.snapshot(&state.1)
    }
    pub fn stage(&self, request: &ChangeRequest) -> Result<StagedChange, SemanticError> {
        self.stage_with(request, |_| Ok(BTreeSet::new()))
    }
    /// Trusted host checks may execute tests/verifiers and return the check names
    /// actually completed. A request cannot choose this callback or its result.
    pub fn stage_with(
        &self,
        request: &ChangeRequest,
        checks: impl FnOnce(&WorkspaceSnapshot) -> Result<BTreeSet<String>, SemanticError>,
    ) -> Result<StagedChange, SemanticError> {
        let (data, policy) = self
            .state
            .lock()
            .map_err(|_| invalid("workspace lock poisoned"))?
            .clone();
        stage(&data, &policy, request, checks)
    }
    pub fn commit(
        &self,
        staged: StagedChange,
        approval: Option<&Approval>,
    ) -> Result<CommitReceipt, SemanticError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| invalid("workspace lock poisoned"))?;
        // A single replacement publishes every byte and ledger change together.
        let receipt = commit(&state.0, &state.1, &staged, approval)?;
        if !state.0.outcomes.contains_key(&staged.request.request_id) {
            let mut candidate = staged.candidate;
            candidate.outcomes = state.0.outcomes.clone();
            candidate.outcomes.insert(
                staged.request.request_id.clone(),
                RecordedOutcome {
                    payload: staged.payload,
                    receipt: receipt.clone(),
                },
            );
            state.0 = candidate;
        }
        Ok(receipt)
    }
    /// Trusted owner baseline advancement; never callable through ChangeRequest.
    pub fn install_baseline(&self, policy: TrustedBaseline) -> Result<(), SemanticError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| invalid("workspace lock poisoned"))?;
        let snapshot = state.0.snapshot(&policy)?;
        policy.check(&snapshot.files, &snapshot.index)?;
        state.1 = policy;
        Ok(())
    }
}

fn stage(
    data: &WorkspaceData,
    policy: &TrustedBaseline,
    request: &ChangeRequest,
    checks: impl FnOnce(&WorkspaceSnapshot) -> Result<BTreeSet<String>, SemanticError>,
) -> Result<StagedChange, SemanticError> {
    request.validate()?;
    let payload = digest(serde_json::to_vec(request).map_err(json_error)?);
    if let Some(recorded) = data.outcomes.get(&request.request_id) {
        if recorded.payload != payload {
            return Err(invalid("request ID reused with a different payload"));
        }
        return Ok(StagedChange {
            request: request.clone(),
            payload,
            candidate: data.clone(),
            receipt: recorded.receipt.clone(),
        });
    }
    let base = data.snapshot(policy)?;
    if base.revision != request.base_revision {
        return Err(stale());
    }
    if base.policy_baseline != request.expected_policy_baseline {
        return Err(policy_mismatch());
    }
    // Pending approved specification contents cannot silently become an ordinary
    // implementation baseline, even if the next edit leaves them untouched.
    if request.transaction == Transaction::Implementation {
        policy.check(&base.files, &base.index)?;
    }
    let mut candidate = data.clone();
    let mut edits = Vec::<(SourceRange, String)>::new();
    for operation in &request.operations {
        let declaration = base.index.find(operation.target_id())?;
        if declaration.declaration_hash != operation.expected_hash() {
            return Err(SemanticError::new(
                "PTN_CHANGE_STALE_TARGET",
                format!("declaration {} changed", declaration.id),
            ));
        }
        match operation {
            Operation::ReplaceFunctionBody { body, .. } => {
                let range = declaration
                    .body_range
                    .clone()
                    .ok_or_else(|| invalid("body replacement requires a function"))?;
                if !body.trim().starts_with('{') || !body.trim().ends_with('}') {
                    return Err(invalid(
                        "replacement must contain a complete brace-delimited body",
                    ));
                }
                edits.push((range, body.clone()));
            }
            Operation::RenameSymbol { new_name, .. } => {
                if declaration.kind != "function" {
                    return Err(invalid("only function renames are supported"));
                }
                if declaration.public && !policy.allow_public_rename {
                    return Err(invalid(
                        "public rename requires a trusted complete consumer inventory",
                    ));
                }
                edits.push((declaration.name_range.clone(), new_name.clone()));
                for reference in base
                    .index
                    .references
                    .iter()
                    .filter(|r| r.target == declaration.id)
                {
                    let mut range = reference.range.clone();
                    // Qualified references retain the module prefix.
                    range.start_byte = range
                        .end_byte
                        .checked_sub(declaration.name.len())
                        .ok_or_else(|| invalid("uneditable reference"))?;
                    let bytes = base
                        .files
                        .get(&range.path)
                        .ok_or_else(|| invalid("uneditable external reference"))?;
                    if bytes.get(range.start_byte..range.end_byte)
                        != Some(declaration.name.as_bytes())
                    {
                        return Err(invalid(
                            "reference does not end in the resolved function name",
                        ));
                    }
                    edits.push((range, new_name.clone()));
                }
                candidate
                    .ledger
                    .identities
                    .iter_mut()
                    .find(|i| i.id == declaration.id)
                    .ok_or_else(|| invalid("missing rename identity"))?
                    .name = new_name.clone();
            }
            Operation::ReplacePact {
                declaration: replacement,
                ..
            } => {
                if declaration.kind != "pact" {
                    return Err(invalid("pact replacement requires a pact"));
                }
                edits.push((declaration.range.clone(), replacement.clone()));
            }
        }
    }
    edits.sort_by(|a, b| (&a.0.path, a.0.start_byte).cmp(&(&b.0.path, b.0.start_byte)));
    for pair in edits.windows(2) {
        if pair[0].0.path == pair[1].0.path && pair[0].0.end_byte > pair[1].0.start_byte {
            return Err(invalid("operation footprints overlap"));
        }
    }
    for (range, replacement) in edits.into_iter().rev() {
        let bytes = candidate
            .files
            .get_mut(&range.path)
            .ok_or_else(|| invalid("editable source is missing"))?;
        let mut text =
            String::from_utf8(bytes.clone()).map_err(|_| invalid("source is not UTF-8"))?;
        if !text.is_char_boundary(range.start_byte) || !text.is_char_boundary(range.end_byte) {
            return Err(invalid("edit does not lie on UTF-8 boundaries"));
        }
        text.replace_range(range.start_byte..range.end_byte, &replacement);
        *bytes = text.into_bytes();
    }
    if candidate.files == data.files {
        return Err(invalid("no-op request"));
    }
    let source = source_snapshot(&candidate.files)?;
    let diagnostics = source.check();
    if !diagnostics.is_empty() {
        return Err(invalid(format!(
            "candidate failed frontend checks: {}",
            serde_json::to_string(&diagnostics).map_err(json_error)?
        )));
    }
    let index = SemanticIndex::build(&source, &candidate.ledger)?;
    if index.declarations.len() != base.index.declarations.len() {
        return Err(invalid(
            "operation inserted or deleted declarations outside its footprint",
        ));
    }
    for before in &base.index.declarations {
        let after = index
            .find(&before.id)
            .map_err(|_| invalid("operation changed declaration identity"))?;
        let operation = request
            .operations
            .iter()
            .find(|o| o.target_id() == before.id);
        if before.kind != after.kind
            || before.module != after.module
            || before.public != after.public
        {
            return Err(invalid(
                "operation changed declaration kind, module, or visibility",
            ));
        }
        match operation {
            Some(Operation::RenameSymbol { new_name, .. }) if after.name == *new_name => {}
            _ if before.name != after.name => {
                return Err(invalid("operation changed a name outside its footprint"));
            }
            _ => {}
        }
        if before.kind == "function"
            && !matches!(operation, Some(Operation::RenameSymbol { .. }))
            && before.signature != after.signature
        {
            return Err(invalid("body replacement changed a signature/effect bound"));
        }
        if before.kind == "pact"
            && pact_target(&base.index, &before.id) != pact_target(&index, &after.id)
        {
            return Err(invalid("pact replacement changed the pact target"));
        }
    }
    candidate.ledger = index.ledger;
    let snapshot = candidate.snapshot(policy)?;
    if request.transaction == Transaction::Implementation {
        policy.check(&snapshot.files, &snapshot.index)?;
    }
    let mut performed_checks = checks(&snapshot)?;
    performed_checks.insert("frontend".into());
    if !policy.required_checks().is_subset(&performed_checks) {
        return Err(invalid("required trusted checks have not all completed"));
    }
    let receipt = CommitReceipt {
        request_id: request.request_id.clone(),
        base_revision: base.revision,
        candidate_revision: snapshot.revision,
        policy_baseline: base.policy_baseline,
        diff: exact_diff(&base.files, &snapshot.files),
        performed_checks,
        baseline_advancement_pending: request.transaction == Transaction::Specification,
    };
    Ok(StagedChange {
        request: request.clone(),
        payload,
        candidate,
        receipt,
    })
}

fn commit(
    data: &WorkspaceData,
    policy: &TrustedBaseline,
    staged: &StagedChange,
    approval: Option<&Approval>,
) -> Result<CommitReceipt, SemanticError> {
    if let Some(recorded) = data.outcomes.get(&staged.request.request_id) {
        if recorded.payload == staged.payload {
            return Ok(recorded.receipt.clone());
        }
        return Err(invalid("request ID reused with a different payload"));
    }
    let current = data.snapshot(policy)?;
    if current.revision != staged.receipt.base_revision {
        return Err(stale());
    }
    if current.policy_baseline != staged.receipt.policy_baseline {
        return Err(policy_mismatch());
    }
    if staged.request.transaction == Transaction::Specification
        && !approval.is_some_and(|a| {
            a.matches(
                &staged.receipt.base_revision,
                &staged.receipt.candidate_revision,
                &staged.receipt.policy_baseline,
                &staged.receipt.diff,
            )
        })
    {
        return Err(SemanticError::new(
            "PTN_CHANGE_APPROVAL_REQUIRED",
            "specification commit requires current external approval for the exact revisions, baseline, and complete diff",
        ));
    }
    Ok(staged.receipt.clone())
}

fn pact_target(index: &SemanticIndex, id: &str) -> Option<String> {
    let declaration = index.find(id).ok()?;
    let header_end = crate::semantic::pact_body_start(declaration)?;
    index
        .references
        .iter()
        .find(|r| r.owner == id && r.range.start_byte < header_end)
        .map(|r| r.target.clone())
}

pub fn source_snapshot(files: &BTreeMap<String, Vec<u8>>) -> Result<SourceSnapshot, SemanticError> {
    let mut folded = BTreeSet::new();
    for path in files.keys() {
        if !valid_path(path) || !folded.insert(path.to_lowercase()) {
            return Err(invalid(format!("invalid workspace path {path:?}")));
        }
    }
    let inputs = files
        .iter()
        .filter(|(path, _)| path.ends_with(".ptn"))
        .map(|(path, bytes)| SourceInput {
            path: path.clone(),
            bytes: bytes.clone(),
            kind: if path.ends_with(".pact.ptn") {
                FileKind::Contract
            } else {
                FileKind::Implementation
            },
        })
        .collect();
    SourceSnapshot::new(inputs).map_err(|d| invalid(d.to_string()))
}

fn invalid(message: impl Into<String>) -> SemanticError {
    SemanticError::new("PTN_CHANGE_INVALID_REQUEST", message)
}
fn stale() -> SemanticError {
    SemanticError::new(
        "PTN_CHANGE_STALE_REVISION",
        "workspace changed since the base snapshot",
    )
}
fn policy_mismatch() -> SemanticError {
    SemanticError::new(
        "PTN_CHANGE_POLICY_MISMATCH",
        "external policy baseline changed",
    )
}

/// Persistent single-object store. The trusted host exclusively owns this
/// directory; participating clients must use this API, not edit store files.
pub struct ManagedWorkspace {
    directory: PathBuf,
}
impl ManagedWorkspace {
    pub fn create(
        directory: impl AsRef<Path>,
        data: &WorkspaceData,
    ) -> Result<Self, SemanticError> {
        let directory = directory.as_ref();
        std::fs::create_dir(directory).map_err(io_error)?;
        let store = Self {
            directory: directory.into(),
        };
        let _lock = store.lock()?;
        store.write(data)?;
        Ok(store)
    }
    pub fn open(directory: impl AsRef<Path>) -> Result<Self, SemanticError> {
        let store = Self {
            directory: directory.as_ref().into(),
        };
        let _lock = store.lock()?;
        store.read()?;
        Ok(store)
    }
    pub fn snapshot(&self, policy: &TrustedBaseline) -> Result<WorkspaceSnapshot, SemanticError> {
        let _lock = self.lock()?;
        self.read()?.snapshot(policy)
    }
    pub fn stage(
        &self,
        policy: &TrustedBaseline,
        request: &ChangeRequest,
    ) -> Result<StagedChange, SemanticError> {
        self.stage_with(policy, request, |_| Ok(BTreeSet::new()))
    }
    pub fn stage_with(
        &self,
        policy: &TrustedBaseline,
        request: &ChangeRequest,
        checks: impl FnOnce(&WorkspaceSnapshot) -> Result<BTreeSet<String>, SemanticError>,
    ) -> Result<StagedChange, SemanticError> {
        let data = {
            let _lock = self.lock()?;
            self.read()?
        };
        stage(&data, policy, request, checks)
    }
    /// The host must re-read its trusted policy/approval at commit to observe
    /// revocation; they must never be loaded from candidate-owned files.
    pub fn commit(
        &self,
        policy: &TrustedBaseline,
        staged: StagedChange,
        approval: Option<&Approval>,
    ) -> Result<CommitReceipt, SemanticError> {
        let _lock = self.lock()?;
        let data = self.read()?;
        let receipt = commit(&data, policy, &staged, approval)?;
        if !data.outcomes.contains_key(&staged.request.request_id) {
            let mut candidate = staged.candidate;
            candidate.outcomes = data.outcomes;
            candidate.outcomes.insert(
                staged.request.request_id,
                RecordedOutcome {
                    payload: staged.payload,
                    receipt: receipt.clone(),
                },
            );
            self.write(&candidate)?;
        }
        Ok(receipt)
    }
    fn lock(&self) -> Result<std::fs::File, SemanticError> {
        // Reject symlink components and aliases. The trusted ownership boundary
        // prevents replacement races; advisory locks coordinate all store users.
        let mut component_path = PathBuf::new();
        for component in self.directory.components() {
            if matches!(component, std::path::Component::ParentDir) {
                return Err(invalid("managed path contains parent traversal"));
            }
            component_path.push(component);
            if std::fs::symlink_metadata(&component_path)
                .map_err(io_error)?
                .file_type()
                .is_symlink()
            {
                return Err(invalid("managed path contains a symlink"));
            }
        }
        if !self.directory.is_dir() {
            return Err(invalid("managed store is not a directory"));
        }
        let path = self.directory.join("lock");
        if path.exists() {
            regular_file(&path)?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(io_error)?;
        file.lock().map_err(io_error)?;
        Ok(file)
    }
    fn read(&self) -> Result<WorkspaceData, SemanticError> {
        let path = self.directory.join("workspace.json");
        regular_file(&path)?;
        serde_json::from_slice(&std::fs::read(path).map_err(io_error)?).map_err(json_error)
    }
    fn write(&self, data: &WorkspaceData) -> Result<(), SemanticError> {
        use std::io::Write;
        let target = self.directory.join("workspace.json");
        if target.exists() {
            regular_file(&target)?;
        }
        let temporary = self.directory.join(format!(
            "snapshot-{}.tmp",
            digest(serde_json::to_vec(data).map_err(json_error)?).replace(':', "-")
        ));
        // An interrupted publication leaves only an unreferenced temporary file.
        // It is safe to remove under the exclusive store lock after validation.
        if temporary.exists() {
            regular_file(&temporary)?;
            std::fs::remove_file(&temporary).map_err(io_error)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(io_error)?;
        file.write_all(&serde_json::to_vec(data).map_err(json_error)?)
            .map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        std::fs::rename(&temporary, target).map_err(io_error)?;
        std::fs::File::open(&self.directory)
            .and_then(|directory| directory.sync_all())
            .map_err(io_error)?;
        Ok(())
    }
}

fn regular_file(path: &Path) -> Result<(), SemanticError> {
    let metadata = std::fs::symlink_metadata(path).map_err(io_error)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("managed store input is not a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(invalid("managed store input is multiply linked"));
        }
    }
    Ok(())
}
fn io_error(error: std::io::Error) -> SemanticError {
    SemanticError::new("PTN_WORKSPACE_IO", error.to_string())
}

// serde_json::Value normally overwrites duplicate object keys. This visitor
// rejects them at every nesting depth before ChangeRequest deserialization.
struct UniqueValue(serde_json::Value);
pub(crate) fn decode_strict<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
) -> Result<T, SemanticError> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = UniqueValue::deserialize(&mut decoder)
        .map_err(json_error)?
        .0;
    decoder.end().map_err(json_error)?;
    serde_json::from_value(value).map_err(json_error)
}
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON with unique object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(UniqueValue(serde_json::Value::Number(
                    serde_json::Number::from_f64(v).ok_or_else(|| E::custom("nonfinite number"))?,
                )))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(serde_json::Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = vec![];
                while let Some(value) = seq.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(values.into()))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate JSON key {key:?}"
                        )));
                    }
                    values.insert(key, map.next_value::<UniqueValue>()?.0);
                }
                Ok(UniqueValue(values.into()))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
