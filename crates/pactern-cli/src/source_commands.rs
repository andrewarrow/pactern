use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use pactern::frontend::{SourceInput, SourceSnapshot};
use pactern::syntax::{FileKind, MAX_SOURCE_BYTES, SourceFile};
use pactern::{Diagnostic, formatting};
use serde::Serialize;

const MAX_FILES: usize = 256;
const MAX_PACKAGE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Serialize)]
pub struct SourceOutput {
    cli_version: &'static str,
    kind: &'static str,
    source_revision: String,
    check_only: bool,
    files: Vec<FileOutput>,
}

#[derive(Serialize)]
struct FileOutput {
    path: String,
    changed: bool,
}

impl SourceOutput {
    pub fn summary(&self) -> String {
        if self.kind == "check" {
            format!(
                "Checked {} source files (core value types).",
                self.files.len()
            )
        } else if self.check_only {
            format!("Checked formatting of {} source files.", self.files.len())
        } else {
            format!(
                "Formatted {} of {} source files.",
                self.files.iter().filter(|file| file.changed).count(),
                self.files.len()
            )
        }
    }
}

pub fn run(paths: &[PathBuf], format: bool, check: bool) -> Result<SourceOutput, Diagnostic> {
    let root = fs::canonicalize(std::env::current_dir().map_err(io_error)?).map_err(io_error)?;
    let mut discovered = Vec::new();
    if paths.is_empty() {
        for directory in ["src", "contracts"] {
            let path = root.join(directory);
            if path.try_exists().map_err(io_error)? {
                discover(&root, &path, 0, &mut discovered)?;
            }
        }
    } else {
        for path in paths {
            discover(&root, &root.join(path), 0, &mut discovered)?;
        }
    }
    if discovered.is_empty() {
        return Err(Diagnostic::no_sources());
    }
    discovered.sort();
    if discovered.windows(2).any(|paths| paths[0] == paths[1]) {
        return Err(Diagnostic::invalid_arguments(
            "Source selections overlap; supply each file once.",
        ));
    }
    let mut total = 0;
    let mut inputs = Vec::new();
    for path in discovered {
        let name = relative_name(&root, &path)?;
        // A bounded read also covers files growing after metadata was inspected.
        let mut bytes = Vec::new();
        File::open(&path)
            .map_err(io_error)?
            .take((MAX_SOURCE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > MAX_SOURCE_BYTES {
            return Err(Diagnostic::source_limit(format!(
                "{name} exceeds the {MAX_SOURCE_BYTES}-byte source limit."
            )));
        }
        total += bytes.len();
        if total > MAX_PACKAGE_BYTES {
            return Err(Diagnostic::source_limit(
                "Source package exceeds the 16 MiB limit.",
            ));
        }
        let kind = if name.ends_with(".pact.ptn") {
            FileKind::Contract
        } else {
            FileKind::Implementation
        };
        inputs.push(SourceInput {
            path: name,
            bytes,
            kind,
        });
    }
    let snapshot = SourceSnapshot::new(inputs)?;
    if !format && let Some(error) = snapshot.check().into_iter().next() {
        return Err(error);
    }
    let mut formatted = Vec::new();
    if format {
        // Validate every selected file before any write, including UTF-8 failures.
        for input in snapshot.inputs() {
            let source = SourceFile::from_bytes(&input.path, &input.bytes)
                .map_err(|error| snapshot.syntax_diagnostic(&input.path, &error))?;
            let output = formatting::format(&source, input.kind)
                .map_err(|error| snapshot.syntax_diagnostic(&input.path, &error))?;
            formatted.push(output);
        }
        if check {
            for (input, output) in snapshot.inputs().iter().zip(&formatted) {
                if input.bytes != output.as_bytes() {
                    return Err(snapshot.formatting_diagnostic(&input.path));
                }
            }
        }
    }
    let mut files = Vec::new();
    for (index, input) in snapshot.inputs().iter().enumerate() {
        let changed = format && input.bytes != formatted[index].as_bytes();
        if changed {
            replace(
                &root.join(&input.path),
                &input.bytes,
                formatted[index].as_bytes(),
            )?;
        }
        files.push(FileOutput {
            path: input.path.clone(),
            changed,
        });
    }
    Ok(SourceOutput {
        cli_version: pactern::CLI_VERSION,
        kind: if format { "format" } else { "check" },
        source_revision: snapshot.revision().to_owned(),
        check_only: !format || check,
        files,
    })
}

fn relative_name(root: &Path, path: &Path) -> Result<String, Diagnostic> {
    let relative = path.strip_prefix(root).map_err(|_| {
        Diagnostic::invalid_arguments("Source paths must be inside the current workspace.")
    })?;
    let mut parts = Vec::new();
    for component in relative.components() {
        parts.push(
            component.as_os_str().to_str().ok_or_else(|| {
                Diagnostic::invalid_arguments("Source paths must be valid UTF-8.")
            })?,
        );
    }
    let name = parts.join("/");
    if !pactern::frontend::valid_path(&name) {
        return Err(Diagnostic::invalid_arguments(
            "Source paths must use canonical protocol path characters.",
        ));
    }
    Ok(name)
}

fn discover(
    root: &Path,
    path: &Path,
    depth: usize,
    out: &mut Vec<PathBuf>,
) -> Result<(), Diagnostic> {
    if depth > 64 {
        return Err(Diagnostic::source_limit(
            "Source directory nesting exceeds 64.",
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        return Err(Diagnostic::invalid_arguments(
            "Source selections must not contain symbolic links.",
        ));
    }
    let path = fs::canonicalize(path).map_err(io_error)?;
    if !path.starts_with(root) {
        return Err(Diagnostic::invalid_arguments(
            "Source paths must be inside the current workspace.",
        ));
    }
    if metadata.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(&path)
            .map_err(io_error)?
            .collect::<Result<_, _>>()
            .map_err(io_error)?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let kind = entry.file_type().map_err(io_error)?;
            if kind.is_dir()
                || kind.is_symlink()
                || entry.path().extension().is_some_and(|ext| ext == "ptn")
            {
                discover(root, &entry.path(), depth + 1, out)?;
            }
        }
    } else if metadata.is_file() && path.extension().is_some_and(|ext| ext == "ptn") {
        relative_name(root, &path)?;
        out.push(path);
        if out.len() > MAX_FILES {
            return Err(Diagnostic::source_limit(
                "Source selection exceeds 256 files.",
            ));
        }
    } else {
        return Err(Diagnostic::invalid_arguments(
            "Select regular .ptn source files or directories.",
        ));
    }
    Ok(())
}

// Each file replacement is atomic; a multi-file format is not a transaction.
fn replace(path: &Path, expected: &[u8], bytes: &[u8]) -> Result<(), Diagnostic> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| Diagnostic::io_error("Source has no parent directory."))?;
    let temporary = parent.join(format!(
        ".pactern-fmt-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_error)?;
    let result = (|| {
        let metadata = fs::symlink_metadata(path).map_err(io_error)?;
        if !metadata.is_file() || fs::read(path).map_err(io_error)? != expected {
            return Err(Diagnostic::io_error(
                "Source changed while formatting; rerun the command.",
            ));
        }
        file.set_permissions(metadata.permissions())
            .map_err(io_error)?;
        file.write_all(bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        drop(file);
        fs::rename(&temporary, path).map_err(io_error)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn io_error(error: std::io::Error) -> Diagnostic {
    Diagnostic::io_error(error.to_string())
}
