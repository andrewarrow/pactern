//! Compile this executable from an externally pinned baseline checkout. It does
//! not load candidate policy, run candidate scripts, or trust candidate test lists.
//! Usage: acceptance_gate BASELINE_ROOT CANDIDATE_ROOT [TRUSTED_APPROVAL_JSON]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use pactern::acceptance::{Approval, HashRecord, exact_diff, inventory_hash};
use pactern::semantic::digest;
use serde::Serialize;

const PROTECTED_ROOTS: &[&str] = &[
    "docs/spec/",
    "fixtures/",
    "baselines/",
    ".github/workflows/",
    "scripts/",
];
const MAX_BYTES: usize = 128 * 1024 * 1024;

#[derive(Serialize)]
struct Report {
    protocol_version: &'static str,
    kind: &'static str,
    accepted: bool,
    base_revision: String,
    candidate_revision: String,
    policy_baseline: String,
    protected_changes: Vec<String>,
    diff: Vec<pactern::acceptance::FileDiff>,
    baseline_advancement_pending: bool,
}

fn main() {
    match run() {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("trusted acceptance gate: {error}");
            std::process::exit(2);
        }
    }
}

fn run() -> Result<bool, Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if !(2..=3).contains(&arguments.len()) {
        return Err(
            "usage: acceptance_gate BASELINE_ROOT CANDIDATE_ROOT [TRUSTED_APPROVAL_JSON]".into(),
        );
    }
    let baseline_root = Path::new(&arguments[0]).canonicalize()?;
    let candidate_root = Path::new(&arguments[1]).canonicalize()?;
    let baseline = inventory(&baseline_root)?;
    let candidate = inventory(&candidate_root)?;
    let executable_hash = digest(std::fs::read(std::env::current_exe()?)?);
    let mut policy_records = vec![
        HashRecord {
            category: "tool".into(),
            key: "externally-built-acceptance-gate".into(),
            digest: executable_hash.clone(),
        },
        HashRecord {
            category: "configuration".into(),
            key: "protected-inventory-rules".into(),
            digest: digest(serde_json::to_vec(&(
                PROTECTED_ROOTS,
                "all crate tests and baseline harness; no symlinks or hardlinks; all eligible source/config files",
            ))?),
        },
    ];
    // Candidate classification may add protected changes but must never choose
    // the accepted bytes that define the externally fixed baseline digest.
    let baseline_protected = protected_paths(&baseline, &BTreeMap::new());
    let protected_paths = protected_paths(&baseline, &candidate);
    for (path, bytes) in baseline
        .iter()
        .filter(|(path, _)| baseline_protected.contains(*path))
    {
        policy_records.push(HashRecord {
            category: "file".into(),
            key: path.clone(),
            digest: digest(bytes),
        });
    }
    let policy_baseline = inventory_hash("policy", &policy_records)?;
    let revision = |files: &BTreeMap<String, Vec<u8>>| {
        let mut records: Vec<_> = files
            .iter()
            .map(|(path, bytes)| HashRecord {
                category: "file".into(),
                key: path.clone(),
                digest: digest(bytes),
            })
            .collect();
        records.push(HashRecord {
            category: "policy".into(),
            key: "baseline".into(),
            digest: policy_baseline.clone(),
        });
        records.push(HashRecord {
            category: "tool".into(),
            key: "externally-built-acceptance-gate".into(),
            digest: executable_hash.clone(),
        });
        inventory_hash("workspace", &records)
    };
    let base_revision = revision(&baseline)?;
    let candidate_revision = revision(&candidate)?;
    let diff = exact_diff(&baseline, &candidate);
    let protected_changes: Vec<_> = diff
        .iter()
        .filter(|change| protected_paths.contains(&change.path))
        .map(|change| change.path.clone())
        .collect();
    let approved = if let Some(path) = arguments.get(2) {
        let path = Path::new(path).canonicalize()?;
        if path.starts_with(&candidate_root) || path.starts_with(&baseline_root) {
            return Err(
                "approval must come from trusted owner storage outside both checkouts".into(),
            );
        }
        let approval: Approval = serde_json::from_slice(&std::fs::read(path)?)?;
        approval.matches(&base_revision, &candidate_revision, &policy_baseline, &diff)
    } else {
        false
    };
    let accepted = protected_changes.is_empty() || approved;
    let pending = approved && !protected_changes.is_empty();
    println!(
        "{}",
        serde_json::to_string_pretty(&Report {
            protocol_version: pactern::PROTOCOL_VERSION,
            kind: "acceptance_gate",
            accepted,
            base_revision,
            candidate_revision,
            policy_baseline,
            protected_changes,
            diff,
            baseline_advancement_pending: pending
        })?
    );
    Ok(accepted)
}

fn protected(path: &str) -> bool {
    PROTECTED_ROOTS.iter().any(|root| path.starts_with(root))
        || path.starts_with("crates/") && path.contains("/tests/")
        || path.starts_with("crates/pactern-baseline/")
        || path == "crates/pactern/examples/acceptance_gate.rs"
        || path.ends_with(".pact.ptn")
        || path.split('/').any(|part| part == ".cargo")
        || matches!(
            path.rsplit('/').next(),
            Some(
                "Cargo.toml"
                    | "Cargo.lock"
                    | "build.rs"
                    | "rust-toolchain"
                    | "rust-toolchain.toml"
                    | "nextest.toml"
                    | "clippy.toml"
                    | ".clippy.toml"
            )
        )
}

fn embedded_tests(path: &str, bytes: &[u8]) -> bool {
    if !path.ends_with(".rs") {
        return false;
    }
    // Conservative text recognition intentionally also protects comments and
    // strings containing test attributes. Never try to edit/extract Rust code.
    let compact: String = String::from_utf8_lossy(bytes)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    let has_module_boundary = String::from_utf8_lossy(bytes)
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .any(|token| token == "mod");
    compact.contains("#[test]")
        || compact.contains("::test]")
        || compact.contains("#[cfg") && compact.contains("test")
        || has_module_boundary
        || compact.contains("include!")
}

fn protected_paths(
    baseline: &BTreeMap<String, Vec<u8>>,
    candidate: &BTreeMap<String, Vec<u8>>,
) -> BTreeSet<String> {
    baseline
        .iter()
        .chain(candidate)
        .filter(|(path, bytes)| protected(path) || embedded_tests(path, bytes))
        .map(|(path, _)| path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifests_toolchains_and_embedded_tests_cannot_erase_required_checks() {
        for path in [
            "Cargo.toml",
            "nested/Cargo.toml",
            "Cargo.lock",
            "nested/build.rs",
            "rust-toolchain.toml",
            ".cargo/config.toml",
            "nested/.cargo/config",
            ".config/nextest.toml",
        ] {
            assert!(protected(path), "unprotected suite configuration: {path}");
        }
        let baseline = BTreeMap::from([
            ("Cargo.toml".into(), b"[[test]]\nname = 'required'".to_vec()),
            (
                "crates/core/src/lib.rs".into(),
                b"#[cfg ( test )]\nmod tests { #[test] fn invariant() {} }".to_vec(),
            ),
        ]);
        let candidate = BTreeMap::from([
            ("Cargo.toml".into(), b"autotests = false".to_vec()),
            (
                "crates/core/src/lib.rs".into(),
                b"// tests removed".to_vec(),
            ),
        ]);
        let protected = protected_paths(&baseline, &candidate);
        assert_eq!(
            exact_diff(&baseline, &candidate)
                .iter()
                .filter(|change| protected.contains(&change.path))
                .count(),
            2
        );
    }
}

fn inventory(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut files = BTreeMap::new();
    let mut folded = BTreeSet::new();
    let mut pending = vec![root.to_owned()];
    let mut bytes = 0;
    while let Some(directory) = pending.pop() {
        let mut entries = std::fs::read_dir(&directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if matches!(
                entry.file_name().to_str(),
                Some(".git" | "target" | ".DS_Store" | "node_modules")
            ) {
                continue;
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err(format!("symlink input is forbidden: {}", path.display()).into());
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(format!("nonregular input: {}", path.display()).into());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() != 1 {
                    return Err(format!("multiply linked input: {}", path.display()).into());
                }
            }
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .ok_or("non-UTF-8 path")?
                .replace(std::path::MAIN_SEPARATOR, "/");
            if !pactern::frontend::valid_path(&relative) || !folded.insert(relative.to_lowercase())
            {
                return Err(format!("invalid or aliased input path {relative:?}").into());
            }
            bytes += usize::try_from(metadata.len())?;
            if bytes > MAX_BYTES || files.len() >= 20_000 {
                return Err("acceptance input inventory exceeds its bounded profile".into());
            }
            files.insert(relative, std::fs::read(path)?);
        }
    }
    Ok(files)
}
