//! Editor-facing navigation and UTF-16 positions over the shared semantic index.
//! Clients retain the revision and must discard stale responses.

use serde::{Deserialize, Serialize};

use crate::frontend::SourceSnapshot;
use crate::semantic::{QueryResult, SemanticError, SemanticIndex, SourceRange};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,
    pub character: usize,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
#[derive(Clone, Debug, Serialize)]
pub struct EditorDiagnostic {
    pub code: String,
    pub message: String,
    pub severity: String,
    pub path: Option<String>,
    pub range: Option<Range>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum EditorQuery {
    Diagnostics,
    Definition { path: String, byte: usize },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditorRequest {
    pub protocol_version: String,
    pub kind: String,
    pub source_revision: String,
    pub query: EditorQuery,
}
impl EditorRequest {
    pub fn decode(bytes: &[u8]) -> Result<Self, SemanticError> {
        let request: Self = crate::workspace::decode_strict(bytes)?;
        request.validate()?;
        Ok(request)
    }
    fn validate(&self) -> Result<(), SemanticError> {
        if self.protocol_version != crate::PROTOCOL_VERSION
            || self.kind != "editor_request"
            || !crate::acceptance::valid_digest(&self.source_revision)
        {
            return Err(invalid("invalid editor request version, kind, or revision"));
        }
        if let EditorQuery::Definition { path, byte } = &self.query
            && (!crate::frontend::valid_path(path) || *byte as u128 > 9_007_199_254_740_991)
        {
            return Err(invalid("invalid editor path or byte offset"));
        }
        Ok(())
    }
    /// Diagnostics use the source snapshot revision; definition queries use the
    /// supplied index revision, which may be a managed full-workspace revision.
    pub fn execute(
        &self,
        snapshot: &SourceSnapshot,
        index: &SemanticIndex,
    ) -> Result<serde_json::Value, SemanticError> {
        self.validate()?;
        match &self.query {
            EditorQuery::Diagnostics => {
                if self.source_revision != snapshot.revision() {
                    return Err(SemanticError::new(
                        "PTN_CHANGE_STALE_REVISION",
                        "diagnostic request does not match source bytes",
                    ));
                }
                serde_json::to_value(diagnostics(snapshot)?).map_err(crate::acceptance::json_error)
            }
            EditorQuery::Definition { path, byte } => {
                serde_json::to_value(definition(index, &self.source_revision, path, *byte)?)
                    .map_err(crate::acceptance::json_error)
            }
        }
    }
}

/// Zero-based lines and UTF-16 columns, as expected by editor protocols.
pub fn position(text: &str, byte: usize) -> Result<Position, SemanticError> {
    if byte > text.len() || !text.is_char_boundary(byte) {
        return Err(invalid("byte offset is not a UTF-8 scalar boundary"));
    }
    let prefix = &text[..byte];
    let mut line = 0;
    let mut line_start = 0;
    let mut chars = prefix.char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        if ch == '\r' {
            line += 1;
            line_start = offset + 1;
            if chars.peek().is_some_and(|(_, next)| *next == '\n') {
                line_start = chars.next().map(|(n, _)| n + 1).unwrap_or(line_start);
            }
        } else if ch == '\n' {
            line += 1;
            line_start = offset + 1;
        }
    }
    Ok(Position {
        line,
        character: prefix[line_start..].encode_utf16().count(),
    })
}

pub fn diagnostics(
    snapshot: &SourceSnapshot,
) -> Result<QueryResult<Vec<EditorDiagnostic>>, SemanticError> {
    let mut diagnostics = Vec::new();
    for diagnostic in snapshot.check() {
        let location = if let Some(span) = &diagnostic.details.span {
            let input = snapshot
                .inputs()
                .iter()
                .find(|input| input.path == span.path)
                .ok_or_else(|| invalid("diagnostic source disappeared"))?;
            let text =
                std::str::from_utf8(&input.bytes).map_err(|_| invalid("source is not UTF-8"))?;
            Some((
                span.path.clone(),
                Range {
                    start: position(text, span.start_byte)?,
                    end: position(text, span.end_byte)?,
                },
            ))
        } else {
            None
        };
        diagnostics.push(EditorDiagnostic {
            code: diagnostic.code,
            message: diagnostic.message,
            severity: diagnostic.severity.into(),
            path: location.as_ref().map(|(path, _)| path.clone()),
            range: location.map(|(_, range)| range),
        });
    }
    Ok(QueryResult {
        protocol_version: crate::PROTOCOL_VERSION.into(),
        kind: "query_result".into(),
        source_revision: snapshot.revision().into(),
        completeness: "complete".into(),
        reasons: vec![],
        result: diagnostics,
    })
}

pub fn definition(
    index: &SemanticIndex,
    revision: &str,
    path: &str,
    byte: usize,
) -> Result<QueryResult<Option<SourceRange>>, SemanticError> {
    if index.source_revision != revision {
        return Err(SemanticError::new(
            "PTN_CHANGE_STALE_REVISION",
            "editor index does not match the document revision",
        ));
    }
    let target = index
        .references
        .iter()
        .filter(|r| r.range.contains(path, byte))
        .min_by_key(|r| r.range.end_byte - r.range.start_byte)
        .map(|r| r.target.as_str());
    let declaration = target
        .and_then(|id| index.declarations.iter().find(|d| d.id == id))
        .or_else(|| {
            index
                .declarations
                .iter()
                .find(|d| d.name_range.contains(path, byte))
        });
    Ok(QueryResult {
        protocol_version: crate::PROTOCOL_VERSION.into(),
        kind: "query_result".into(),
        source_revision: revision.into(),
        completeness: if index.complete {
            "complete"
        } else {
            "partial"
        }
        .into(),
        reasons: index.reasons.clone(),
        result: declaration.map(|d| d.name_range.clone()),
    })
}

fn invalid(message: &str) -> SemanticError {
    SemanticError::new("PTN_EDITOR_INVALID_POSITION", message)
}
