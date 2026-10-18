//! Rebuildable declaration identities and focused, conservative semantic queries.
//!
//! The ledger is authoritative identity data; the index is disposable. Reconciliation
//! preserves exact locators or a unique token fingerprint. Ambiguous matches fail.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::frontend::SourceSnapshot;
use crate::resolution::{ParsedFile, SymbolId};
use crate::syntax::{DeclarationKind, SourceFile, Span, lex, parse};

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SemanticError {
    pub code: String,
    pub message: String,
}

impl SemanticError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}
impl std::fmt::Display for SemanticError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for SemanticError {}

pub fn digest(bytes: impl AsRef<[u8]>) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes.as_ref()))
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceRange {
    pub path: String,
    pub start_byte: usize,
    pub end_byte: usize,
}
impl SourceRange {
    fn new(path: &str, span: Span) -> Self {
        Self {
            path: path.into(),
            start_byte: span.start.byte,
            end_byte: span.end.byte,
        }
    }
    pub fn contains(&self, path: &str, byte: usize) -> bool {
        self.path == path && self.start_byte <= byte && byte < self.end_byte
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub id: String,
    pub module: String,
    pub name: String,
    pub kind: String,
    pub path: String,
    pub fingerprint: String,
    pub active: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IdentityLedger {
    pub namespace: String,
    pub next_id: u64,
    /// Retired identities remain present and are never allocated again.
    pub identities: Vec<Identity>,
}
impl Default for IdentityLedger {
    fn default() -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let seed = format!(
            "{:?}/{}/{}",
            std::time::SystemTime::now(),
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        );
        Self {
            namespace: digest(seed),
            next_id: 0,
            identities: vec![],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub id: String,
    pub module: String,
    pub name: String,
    pub kind: String,
    pub public: bool,
    pub range: SourceRange,
    pub name_range: SourceRange,
    pub body_range: Option<SourceRange>,
    pub declaration_hash: String,
    pub signature: String,
    pub source: String,
    pub dependencies: BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct Reference {
    pub target: String,
    pub owner: String,
    pub range: SourceRange,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QueryResult<T> {
    pub protocol_version: String,
    pub kind: String,
    pub source_revision: String,
    pub completeness: String,
    pub reasons: Vec<String>,
    pub result: T,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Impact {
    pub declarations: BTreeSet<String>,
    pub interfaces: BTreeSet<String>,
    pub pacts: BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TestDependency {
    pub test: String,
    #[serde(deserialize_with = "unique_dependencies")]
    pub declarations: BTreeSet<String>,
    pub complete: bool,
}

fn unique_dependencies<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<BTreeSet<String>, D::Error> {
    let values = Vec::<String>::deserialize(decoder)?;
    let unique: BTreeSet<_> = values.iter().cloned().collect();
    if values.len() != unique.len() {
        return Err(serde::de::Error::custom("duplicate dependency identity"));
    }
    Ok(unique)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Query {
    Inspect {
        selector: String,
    },
    Callers {
        selector: String,
    },
    Impact {
        selectors: Vec<String>,
    },
    AffectedTests {
        selectors: Vec<String>,
        tests: Vec<TestDependency>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    pub protocol_version: String,
    pub kind: String,
    pub source_revision: String,
    pub query: Query,
}
impl QueryRequest {
    pub fn decode(bytes: &[u8]) -> Result<Self, SemanticError> {
        let request: Self = crate::workspace::decode_strict(bytes)?;
        request.validate()?;
        Ok(request)
    }
    fn validate(&self) -> Result<(), SemanticError> {
        if self.protocol_version != crate::PROTOCOL_VERSION
            || self.kind != "query_request"
            || !crate::acceptance::valid_digest(&self.source_revision)
        {
            return Err(SemanticError::new(
                "PTN_QUERY_INVALID_REQUEST",
                "invalid query version, kind, or revision",
            ));
        }
        let invalid = match &self.query {
            Query::Inspect { selector } | Query::Callers { selector } => selector.is_empty(),
            Query::Impact { selectors } | Query::AffectedTests { selectors, .. } => {
                selectors.is_empty() || selectors.iter().any(String::is_empty)
            }
        };
        if invalid {
            return Err(SemanticError::new(
                "PTN_QUERY_INVALID_REQUEST",
                "queries require nonempty declaration selectors",
            ));
        }
        if let Query::AffectedTests { tests, .. } = &self.query
            && tests.iter().any(|test| {
                test.test.is_empty()
                    || test
                        .declarations
                        .iter()
                        .any(|id| !valid_id(id, "sym_") && !valid_id(id, "pact_"))
            })
        {
            return Err(SemanticError::new(
                "PTN_QUERY_INVALID_REQUEST",
                "test inventory contains an invalid name or identity",
            ));
        }
        Ok(())
    }
    pub fn execute(
        &self,
        index: &SemanticIndex,
    ) -> Result<QueryResult<serde_json::Value>, SemanticError> {
        self.validate()?;
        if self.source_revision != index.source_revision {
            return Err(SemanticError::new(
                "PTN_CHANGE_STALE_REVISION",
                "query revision does not match the semantic index",
            ));
        }
        fn encode<T: Serialize>(
            result: QueryResult<T>,
        ) -> Result<QueryResult<serde_json::Value>, SemanticError> {
            Ok(QueryResult {
                protocol_version: result.protocol_version,
                kind: result.kind,
                source_revision: result.source_revision,
                completeness: result.completeness,
                reasons: result.reasons,
                result: serde_json::to_value(result.result)
                    .map_err(crate::acceptance::json_error)?,
            })
        }
        match &self.query {
            Query::Inspect { selector } => encode(index.inspect(selector)?),
            Query::Callers { selector } => encode(index.callers(selector)?),
            Query::Impact { selectors } => encode(index.impact(selectors)?),
            Query::AffectedTests { selectors, tests } => {
                encode(index.affected_tests(selectors, tests)?)
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SemanticIndex {
    pub source_revision: String,
    pub ledger: IdentityLedger,
    pub declarations: Vec<Declaration>,
    pub references: Vec<Reference>,
    pub complete: bool,
    pub reasons: Vec<String>,
}

impl SemanticIndex {
    pub fn build(
        snapshot: &SourceSnapshot,
        previous: &IdentityLedger,
    ) -> Result<Self, SemanticError> {
        let mut ids = BTreeSet::new();
        for identity in &previous.identities {
            let prefix = if identity.kind == "pact" {
                "pact_"
            } else {
                "sym_"
            };
            if !valid_id(&identity.id, prefix) || !ids.insert(&identity.id) {
                return Err(SemanticError::new(
                    "PTN_IDENTITY_INVALID_LEDGER",
                    "invalid or duplicate persistent identity",
                ));
            }
        }
        let mut files = Vec::new();
        let mut declarations = Vec::new();
        let mut fingerprints = Vec::new();
        for input in snapshot.inputs() {
            let source = SourceFile::from_bytes(&input.path, &input.bytes)
                .map_err(|e| SemanticError::new(e.code, e.message))?;
            let tree =
                parse(&source, input.kind).map_err(|e| SemanticError::new(e.code, e.message))?;
            let module = tree
                .module
                .value
                .value
                .iter()
                .map(|p| p.value.as_str())
                .collect::<Vec<_>>()
                .join("::");
            for declaration in &tree.declarations {
                let (name, kind, body) = match &declaration.kind {
                    DeclarationKind::Function(f) => (&f.name, "function", Some(f.body.span)),
                    DeclarationKind::Struct(s) => (&s.name, "struct", None),
                    DeclarationKind::Enum(e) => (&e.name, "enum", None),
                    DeclarationKind::Pact(p) => (&p.name, "pact", None),
                };
                let text = &source.text()[declaration.span.start.byte..declaration.span.end.byte];
                let mut fingerprint = Sha256::new();
                for token in lex(&SourceFile::new("declaration", text))
                    .map_err(|e| SemanticError::new(e.code, e.message))?
                {
                    let absolute = declaration.span.start.byte + token.span.start.byte;
                    let normalized = if absolute == name.span.start.byte {
                        "<declaration-name>"
                    } else {
                        &token.text
                    };
                    fingerprint.update((normalized.len() as u64).to_be_bytes());
                    fingerprint.update(normalized);
                }
                fingerprints.push(format!("sha256:{:x}", fingerprint.finalize()));
                declarations.push(Declaration {
                    id: String::new(),
                    module: module.clone(),
                    name: name.value.clone(),
                    kind: kind.into(),
                    public: declaration.public,
                    range: SourceRange::new(&input.path, declaration.span),
                    name_range: SourceRange::new(&input.path, name.span),
                    body_range: body.map(|s| SourceRange::new(&input.path, s)),
                    declaration_hash: digest(text),
                    signature: source.text()[declaration.span.start.byte
                        ..body.map_or(declaration.span.end.byte, |s| s.start.byte)]
                        .trim()
                        .into(),
                    source: text.into(),
                    dependencies: BTreeSet::new(),
                });
            }
            files.push(ParsedFile::new(&input.path, tree));
        }
        let resolution = crate::resolution::resolve(&files);
        if let Some(error) = resolution.diagnostics.first() {
            return Err(SemanticError::new(error.code, &error.message));
        }
        let mut ledger = previous.clone();
        for identity in &mut ledger.identities {
            identity.active = false;
        }
        let mut used = BTreeSet::new();
        // Match locators first so new lookalike declarations cannot steal existing IDs.
        for declaration in &mut declarations {
            let matches: Vec<_> = previous
                .identities
                .iter()
                .enumerate()
                .filter(|(_, old)| {
                    old.active
                        && old.module == declaration.module
                        && old.name == declaration.name
                        && old.kind == declaration.kind
                })
                .collect();
            if matches.len() > 1 {
                return Err(ambiguous(&declaration.name));
            }
            if let Some((position, identity)) = matches.first() {
                declaration.id = identity.id.clone();
                used.insert(*position);
            }
        }
        for i in 0..declarations.len() {
            if !declarations[i].id.is_empty() {
                continue;
            }
            let candidates: Vec<_> = previous
                .identities
                .iter()
                .enumerate()
                .filter(|(n, old)| {
                    old.active
                        && !used.contains(n)
                        && old.kind == declarations[i].kind
                        && old.fingerprint == fingerprints[i]
                })
                .collect();
            if candidates.len() > 1 {
                return Err(ambiguous(&declarations[i].name));
            }
            if let Some((position, identity)) = candidates.first() {
                let destinations = declarations
                    .iter()
                    .enumerate()
                    .filter(|(n, d)| {
                        d.id.is_empty()
                            && d.kind == declarations[i].kind
                            && fingerprints[*n] == fingerprints[i]
                    })
                    .count();
                if destinations != 1 {
                    return Err(ambiguous(&declarations[i].name));
                }
                declarations[i].id = identity.id.clone();
                used.insert(*position);
            } else {
                loop {
                    let suffix = digest(format!("{}/{}", ledger.namespace, ledger.next_id));
                    ledger.next_id = ledger.next_id.checked_add(1).ok_or_else(|| {
                        SemanticError::new("PTN_IDENTITY_LIMIT", "identity counter exhausted")
                    })?;
                    let id = format!(
                        "{}_{}",
                        if declarations[i].kind == "pact" {
                            "pact"
                        } else {
                            "sym"
                        },
                        &suffix[7..39]
                    );
                    if !previous.identities.iter().any(|old| old.id == id)
                        && !declarations.iter().any(|d| d.id == id)
                    {
                        declarations[i].id = id;
                        break;
                    }
                }
            }
        }
        for (declaration, fingerprint) in declarations.iter().zip(fingerprints) {
            let updated = Identity {
                id: declaration.id.clone(),
                module: declaration.module.clone(),
                name: declaration.name.clone(),
                kind: declaration.kind.clone(),
                path: declaration.range.path.clone(),
                fingerprint,
                active: true,
            };
            if let Some(old) = ledger
                .identities
                .iter_mut()
                .find(|old| old.id == declaration.id)
            {
                *old = updated;
            } else {
                ledger.identities.push(updated);
            }
        }
        let mut symbols = BTreeMap::<SymbolId, usize>::new();
        for symbol in &resolution.symbols {
            if let Some(location) = symbol.declaration {
                let path = &resolution.files[location.file.0];
                if let Some(index) = declarations.iter().position(|d| {
                    d.name_range.path == *path
                        && d.name_range.start_byte == location.span.start.byte
                        && d.name_range.end_byte == location.span.end.byte
                }) {
                    symbols.insert(symbol.id, index);
                }
            }
        }
        let mut references = Vec::new();
        for reference in &resolution.references {
            let mut symbol = reference.symbol;
            // Field/variant references depend on their owning declaration.
            while !symbols.contains_key(&symbol) {
                if !resolution.symbol(symbol).is_some_and(|s| {
                    matches!(
                        s.kind,
                        crate::resolution::SymbolKind::Field
                            | crate::resolution::SymbolKind::Variant
                    )
                }) {
                    break;
                }
                let Some(parent) = resolution.symbol(symbol).and_then(|s| s.parent) else {
                    break;
                };
                symbol = parent;
            }
            let Some(target_index) = symbols.get(&symbol) else {
                continue;
            };
            let target = declarations[*target_index].id.clone();
            let range = SourceRange::new(
                &resolution.files[reference.location.file.0],
                reference.location.span,
            );
            if let Some(owner) = declarations
                .iter_mut()
                .find(|d| d.range.contains(&range.path, range.start_byte))
            {
                owner.dependencies.insert(target.clone());
                references.push(Reference {
                    target,
                    owner: owner.id.clone(),
                    range,
                });
            }
        }
        let complete = resolution.deferred.is_empty();
        let reasons = if complete {
            vec![]
        } else {
            vec![
                "Deferred field/effect references require conservative dependency selection."
                    .into(),
            ]
        };
        Ok(Self {
            source_revision: snapshot.revision().into(),
            ledger,
            declarations,
            references,
            complete,
            reasons,
        })
    }

    pub fn find(&self, selector: &str) -> Result<&Declaration, SemanticError> {
        let found: Vec<_> = self
            .declarations
            .iter()
            .filter(|d| {
                d.id == selector
                    || format!("{}::{}", d.module, d.name) == selector
                    || d.name == selector
            })
            .collect();
        if found.len() == 1 {
            Ok(found[0])
        } else {
            Err(SemanticError::new(
                "PTN_QUERY_AMBIGUOUS_TARGET",
                format!("selector {selector:?} matches {} declarations", found.len()),
            ))
        }
    }

    fn result<T>(&self, result: T) -> QueryResult<T> {
        QueryResult {
            protocol_version: crate::PROTOCOL_VERSION.into(),
            kind: "query_result".into(),
            source_revision: self.source_revision.clone(),
            completeness: if self.complete { "complete" } else { "partial" }.into(),
            reasons: self.reasons.clone(),
            result,
        }
    }
    pub fn inspect(&self, selector: &str) -> Result<QueryResult<Declaration>, SemanticError> {
        Ok(self.result(self.find(selector)?.clone()))
    }
    pub fn callers(&self, selector: &str) -> Result<QueryResult<Vec<Declaration>>, SemanticError> {
        let id = &self.find(selector)?.id;
        Ok(self.result(
            self.declarations
                .iter()
                .filter(|d| d.kind == "function" && d.dependencies.contains(id))
                .cloned()
                .collect(),
        ))
    }
    pub fn impact(&self, selectors: &[String]) -> Result<QueryResult<Impact>, SemanticError> {
        let mut affected: BTreeSet<_> = selectors
            .iter()
            .map(|s| self.find(s).map(|d| d.id.clone()))
            .collect::<Result<_, _>>()?;
        if !self.complete {
            affected.extend(self.declarations.iter().map(|d| d.id.clone()));
        }
        loop {
            let before = affected.len();
            for d in &self.declarations {
                if d.dependencies.iter().any(|id| affected.contains(id)) {
                    affected.insert(d.id.clone());
                }
            }
            if before == affected.len() {
                break;
            }
        }
        let interfaces = self
            .declarations
            .iter()
            .filter(|d| d.public && affected.contains(&d.id))
            .map(|d| d.id.clone())
            .collect();
        let pacts = self
            .declarations
            .iter()
            .filter(|d| d.kind == "pact" && affected.contains(&d.id))
            .map(|d| d.id.clone())
            .collect();
        Ok(self.result(Impact {
            declarations: affected,
            interfaces,
            pacts,
        }))
    }
    pub fn affected_tests(
        &self,
        selectors: &[String],
        tests: &[TestDependency],
    ) -> Result<QueryResult<Vec<String>>, SemanticError> {
        let impact = self.impact(selectors)?.result;
        let unknown = !self.complete
            || tests.iter().any(|t| {
                !t.complete
                    || t.declarations
                        .iter()
                        .any(|id| !self.declarations.iter().any(|d| &d.id == id))
            });
        let selected = tests
            .iter()
            .filter(|t| {
                unknown
                    || t.declarations
                        .iter()
                        .any(|id| impact.declarations.contains(id))
            })
            .map(|t| t.test.clone())
            .collect();
        let mut result = self.result(selected);
        if unknown {
            result.completeness = "partial".into();
            result.reasons.push(
                "Incomplete test dependency data selects the full supplied test inventory.".into(),
            );
        }
        Ok(result)
    }
}

fn ambiguous(name: &str) -> SemanticError {
    SemanticError::new(
        "PTN_IDENTITY_AMBIGUOUS",
        format!("identity of {name:?} requires explicit review; no identity was guessed"),
    )
}

pub(crate) fn pact_body_start(declaration: &Declaration) -> Option<usize> {
    lex(&SourceFile::new("pact-header", &declaration.source))
        .ok()?
        .into_iter()
        .find(|token| {
            token.kind == crate::syntax::TokenKind::Symbol(crate::syntax::Symbol::LeftBrace)
        })
        .map(|token| declaration.range.start_byte + token.span.start.byte)
}
pub fn valid_id(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix).is_some_and(|s| {
        s.len() == 32
            && s.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}
