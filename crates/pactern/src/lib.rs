//! Shared entry points for the Pactern toolchain.
//!
//! Shared frontend, effects/ownership, typed IR, interpreter, C/Wasm backend,
//! narrow SMT verification, evidence, semantic editing, and scoped host APIs.
//! The CLI adapts project I/O; the metadata-only `execute` dispatcher remains
//! separate from those filesystem and process operations.
//!
//! ```
//! use pactern::{execute, Command, PROTOCOL_VERSION};
//!
//! let version = execute(&Command::Version, PROTOCOL_VERSION)?;
//! assert_eq!(version.protocol_version, "0.1.0");
//! # Ok::<(), pactern::Diagnostic>(())
//! ```

use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

pub mod acceptance;
pub mod backend;
pub mod core;
pub mod editor;
pub mod effects;
pub mod evidence;
pub mod formatting;
pub mod frontend;
pub mod host;
pub mod interpreter;
pub mod pacts;
pub mod resolution;
pub mod semantic;
pub mod syntax;
pub mod typing;
pub mod verification;
pub mod workspace;

pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Build-time commit SHA, or `None` when built without revision metadata.
pub const GIT_SHA: Option<&str> = if env!("PACTERN_BUILD_GIT_SHA").is_empty() {
    None
} else {
    Some(env!("PACTERN_BUILD_GIT_SHA"))
};
pub const CLI_VERSION: &str = "0.1.0";
/// Target specification version; this is not a claim of language conformance.
pub const LANGUAGE_VERSION: &str = "0.1.0";
pub const PROTOCOL_VERSION: &str = "0.1.0";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Version,
    Check,
    Fmt { check: bool },
    Verify,
    Test { affected: bool },
    Build { target: Target },
    Inspect { symbol: String },
    Callers { symbol: String },
    Impact { change: PathBuf },
    Apply { change: PathBuf, dry_run: bool },
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Version => "version",
            Self::Check => "check",
            Self::Fmt { .. } => "fmt",
            Self::Verify => "verify",
            Self::Test { .. } => "test",
            Self::Build { .. } => "build",
            Self::Inspect { .. } => "inspect",
            Self::Callers { .. } => "callers",
            Self::Impact { .. } => "impact",
            Self::Apply { .. } => "apply",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    Wasm,
}

#[derive(Debug, Serialize)]
pub struct VersionInfo {
    pub kind: &'static str,
    pub cli_version: &'static str,
    pub tool_version: &'static str,
    pub git_sha: Option<&'static str>,
    pub language_version: &'static str,
    pub protocol_version: &'static str,
}

pub fn version_info() -> VersionInfo {
    VersionInfo {
        kind: "version",
        cli_version: CLI_VERSION,
        tool_version: TOOL_VERSION,
        git_sha: GIT_SHA,
        language_version: LANGUAGE_VERSION,
        protocol_version: PROTOCOL_VERSION,
    }
}

#[derive(Debug, Serialize)]
pub struct Diagnostic {
    protocol_version: &'static str,
    kind: &'static str,
    code: String,
    severity: &'static str,
    phase: &'static str,
    message: String,
    #[serde(flatten)]
    details: Box<DiagnosticDetails>,
}

#[derive(Debug, Default, Serialize)]
struct DiagnosticDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    source_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span: Option<DiagnosticSpan>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    related: Vec<RelatedDiagnostic>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
}

/// A half-open UTF-8 byte range in a revision's workspace-relative file.
#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticSpan {
    pub path: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct RelatedDiagnostic {
    pub message: String,
    pub span: DiagnosticSpan,
}

impl Diagnostic {
    pub fn operation(
        code: impl Into<String>,
        phase: &'static str,
        message: impl Into<String>,
    ) -> Self {
        let mut diagnostic = Self::new("PTN_TOOL_OPERATION", message);
        diagnostic.code = code.into();
        diagnostic.phase = phase;
        diagnostic
    }

    pub fn invalid_arguments(message: impl Into<String>) -> Self {
        Self::new("PTN_TOOL_INVALID_ARGUMENTS", message)
    }

    pub fn unsupported_protocol(requested: &str) -> Self {
        Self::new(
            "PTN_TOOL_UNSUPPORTED_PROTOCOL",
            format!(
                "Unsupported protocol version '{requested}'; supported version is '{PROTOCOL_VERSION}'."
            ),
        )
    }

    pub fn not_implemented(command: &str) -> Self {
        Self::new(
            "PTN_TOOL_NOT_IMPLEMENTED",
            format!("command is not implemented: {command}"),
        )
    }

    pub fn io_error(message: impl Into<String>) -> Self {
        Self::new("PTN_TOOL_IO", message)
    }

    pub fn no_sources() -> Self {
        Self::new(
            "PTN_TOOL_NO_SOURCES",
            "No .ptn source files found; select files or directories, or add sources under src or contracts.",
        )
    }

    pub fn source_limit(message: impl Into<String>) -> Self {
        Self::new("PTN_TOOL_SOURCE_LIMIT", message)
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn span(&self) -> Option<&DiagnosticSpan> {
        self.details.span.as_ref()
    }

    pub fn related(&self) -> &[RelatedDiagnostic] {
        &self.details.related
    }

    pub fn notes(&self) -> &[String] {
        &self.details.notes
    }

    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            kind: "diagnostic",
            code: code.into(),
            severity: "error",
            phase: "tool",
            message: message.into(),
            details: Box::default(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Diagnostic {}

pub fn validate_protocol_version(requested: &str) -> Result<(), Diagnostic> {
    if requested == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(Diagnostic::unsupported_protocol(requested))
    }
}

/// Dispatch a command against an exact protocol version without filesystem I/O.
///
/// This metadata-only dispatcher remains free of I/O. The CLI adapts source
/// checking and formatting through [`frontend`] and [`formatting`] separately.
pub fn execute(command: &Command, protocol_version: &str) -> Result<VersionInfo, Diagnostic> {
    validate_protocol_version(protocol_version)?;

    match command {
        Command::Version => Ok(version_info()),
        other => Err(Diagnostic::not_implemented(other.name())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn version_dispatch_returns_version_envelope() {
        let version = execute(&Command::Version, PROTOCOL_VERSION).unwrap();
        let value = serde_json::to_value(version).unwrap();

        assert_eq!(value["kind"], "version");
        assert_eq!(value["tool_version"], TOOL_VERSION);
        assert_eq!(value["git_sha"], serde_json::json!(GIT_SHA));
        assert_eq!(value["cli_version"], CLI_VERSION);
        assert_eq!(value["language_version"], LANGUAGE_VERSION);
        assert_eq!(value["protocol_version"], PROTOCOL_VERSION);
    }

    #[test]
    fn protocol_version_must_match_exactly_before_dispatch() {
        let error = execute(&Command::Version, "0.1.1").unwrap_err();
        assert_eq!(error.code(), "PTN_TOOL_UNSUPPORTED_PROTOCOL");
        assert!(execute(&Command::Check, "0.1.0 ").is_err());
    }

    #[test]
    fn unimplemented_commands_return_minimal_diagnostic_without_locations() {
        let error = execute(&Command::Check, PROTOCOL_VERSION).unwrap_err();
        assert_eq!(error.code(), "PTN_TOOL_NOT_IMPLEMENTED");
        assert_eq!(
            error.to_string(),
            "PTN_TOOL_NOT_IMPLEMENTED: command is not implemented: check"
        );

        let value: Value = serde_json::to_value(error).unwrap();
        assert_eq!(value["protocol_version"], PROTOCOL_VERSION);
        assert_eq!(value["kind"], "diagnostic");
        assert_eq!(value["severity"], "error");
        assert_eq!(value["phase"], "tool");
        assert!(value.get("span").is_none());
        assert!(value.get("source_revision").is_none());
        assert!(value.get("related").is_none());
    }
}
