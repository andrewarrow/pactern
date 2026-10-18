//! Source-only frontend snapshots and protocol diagnostic adaptation.
//!
//! Inputs are an explicit closed set: this module performs no filesystem I/O.
//! Snapshot hashes bind diagnostic offsets to exact bytes, file kinds, and the
//! frontend profile. They are not trusted acceptance/evidence revisions: dependency,
//! identity, policy, and distribution inventories belong to later milestones.

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::resolution::{Location, ParsedFile, RelatedLocation};
use crate::syntax::{FileKind, SourceFile, Span, SyntaxDiagnostic, parse};
use crate::{Diagnostic, DiagnosticSpan, RelatedDiagnostic, typing};

#[derive(Clone, Debug)]
pub struct SourceInput {
    pub path: String,
    pub bytes: Vec<u8>,
    pub kind: FileKind,
}

/// Immutable, sorted inputs; locations are always derived from these bytes.
#[derive(Debug)]
pub struct SourceSnapshot {
    inputs: Vec<SourceInput>,
    revision: String,
}

impl SourceSnapshot {
    pub fn new(mut inputs: Vec<SourceInput>) -> Result<Self, Diagnostic> {
        inputs.sort_by(|a, b| a.path.cmp(&b.path));
        let mut paths = BTreeSet::new();
        for input in &inputs {
            if !valid_path(&input.path) {
                return Err(Diagnostic::invalid_arguments(format!(
                    "Invalid workspace-relative source path: {:?}.",
                    input.path
                )));
            }
            if !paths.insert(&input.path) {
                return Err(Diagnostic::invalid_arguments(format!(
                    "Duplicate source path: {}.",
                    input.path
                )));
            }
        }
        // Protocol H framing, scoped to this source-only frontend profile.
        let mut records = Vec::new();
        for input in &inputs {
            records.push(("file", input.path.clone(), Sha256::digest(&input.bytes)));
            let kind = match input.kind {
                FileKind::Implementation => "implementation",
                FileKind::Contract => "contract",
            };
            records.push((
                "configuration",
                format!("file-kind/{}", input.path),
                Sha256::digest(kind),
            ));
        }
        for (key, value) in [
            ("profile", "source-frontend/2-effects-lines"),
            ("language", crate::LANGUAGE_VERSION),
            ("protocol", crate::PROTOCOL_VERSION),
            ("tool-version", crate::TOOL_VERSION),
        ] {
            records.push(("configuration", key.to_owned(), Sha256::digest(value)));
        }
        records.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        let mut hash = Sha256::new();
        hash.update(b"pactern/0.1.0/workspace\n");
        for (category, key, digest) in records {
            for text in [category, key.as_str()] {
                hash.update((text.len() as u64).to_be_bytes());
                hash.update(text.as_bytes());
            }
            hash.update(digest);
        }
        Ok(Self {
            inputs,
            revision: format!("sha256:{:x}", hash.finalize()),
        })
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn inputs(&self) -> &[SourceInput] {
        &self.inputs
    }

    /// Lex and parse all inputs, then resolve and type-check only a complete AST set.
    pub fn check(&self) -> Vec<Diagnostic> {
        let mut files = Vec::new();
        let mut diagnostics = Vec::new();
        for input in &self.inputs {
            match SourceFile::from_bytes(&input.path, &input.bytes)
                .and_then(|source| parse(&source, input.kind))
            {
                Ok(tree) => files.push(ParsedFile::new(&input.path, tree)),
                Err(error) => diagnostics.push(self.syntax_diagnostic(&input.path, &error)),
            }
        }
        if !diagnostics.is_empty() {
            return diagnostics;
        }
        let checked = typing::check(&files);
        for error in &checked.resolution.diagnostics {
            diagnostics.push(self.located(
                error.code,
                error.phase,
                &error.message,
                error.location,
                &error.related,
            ));
        }
        for error in &checked.diagnostics {
            diagnostics.push(self.located(
                error.code,
                error.phase,
                &error.message,
                error.location,
                &error.related,
            ));
        }
        diagnostics
    }

    /// Adapt a lexer/parser error produced against an input in this snapshot.
    pub fn syntax_diagnostic(&self, path: &str, error: &SyntaxDiagnostic) -> Diagnostic {
        let mut diagnostic = Diagnostic::new(error.code, &error.message);
        diagnostic.phase = error.phase;
        diagnostic.details.source_revision = Some(self.revision.clone());
        // Invalid UTF-8 has no protocol scalar boundaries. Keep its byte index in
        // a note instead of advertising a usable UTF-8 source range.
        diagnostic.details.span = self.span(path, error.span);
        if diagnostic.details.span.is_none() {
            diagnostic.details.notes.push(format!(
                "Source: {path}; byte offset: {}.",
                error.span.start.byte
            ));
        }
        diagnostic
    }

    pub fn formatting_diagnostic(&self, path: &str) -> Diagnostic {
        let mut diagnostic =
            Diagnostic::new("PTN_TOOL_FORMAT_REQUIRED", "Source needs formatting.");
        diagnostic.details.source_revision = Some(self.revision.clone());
        if let Some(input) = self.inputs.iter().find(|input| input.path == path) {
            diagnostic.details.span = Some(DiagnosticSpan {
                path: path.to_owned(),
                start_byte: 0,
                end_byte: input.bytes.len(),
            });
        }
        diagnostic
    }

    fn span(&self, path: &str, span: Span) -> Option<DiagnosticSpan> {
        let input = self.inputs.iter().find(|input| input.path == path)?;
        let text = std::str::from_utf8(&input.bytes).ok()?;
        if span.start.byte > span.end.byte
            || span.end.byte > text.len()
            || !text.is_char_boundary(span.start.byte)
            || !text.is_char_boundary(span.end.byte)
        {
            return None;
        }
        Some(DiagnosticSpan {
            path: path.to_owned(),
            start_byte: span.start.byte,
            end_byte: span.end.byte,
        })
    }

    fn located(
        &self,
        code: &'static str,
        phase: &'static str,
        message: &str,
        location: Location,
        related: &[RelatedLocation],
    ) -> Diagnostic {
        let mut diagnostic = Diagnostic::new(code, message);
        diagnostic.phase = phase;
        diagnostic.details.source_revision = Some(self.revision.clone());
        diagnostic.details.span = self
            .inputs
            .get(location.file.0)
            .and_then(|input| self.span(&input.path, location.span));
        diagnostic.details.related = related
            .iter()
            .filter_map(|related| {
                let input = self.inputs.get(related.location.file.0)?;
                Some(RelatedDiagnostic {
                    message: related.message.clone(),
                    span: self.span(&input.path, related.location.span)?,
                })
            })
            .collect();
        diagnostic
    }
}

/// The protocol's canonical workspace-relative path syntax.
pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', ':'])
        && !path.chars().any(|ch| ch <= '\u{1f}' || ch == '\u{7f}')
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}
