use std::path::PathBuf;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output.status.success().then_some(())?;
    Some(String::from_utf8(output.stdout).ok()?.trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=PACTERN_GIT_SHA");

    // Watch both HEAD and its ref: commits can change without switching branches.
    // git-path also resolves linked worktrees and packed refs correctly.
    let mut refs = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    if let Some(head) = git(&["symbolic-ref", "-q", "HEAD"]) {
        refs.push(head);
    }
    for name in refs {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            let mut path = PathBuf::from(path);
            // A packed ref can become loose on the next commit. Watch its
            // directory until the file exists so that creation is detected.
            while !path.exists() && path.pop() {}
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }

    // Source archives can supply the revision without requiring a Git checkout.
    let sha = std::env::var("PACTERN_GIT_SHA")
        .ok()
        .or_else(|| git(&["rev-parse", "--verify", "HEAD"]))
        .unwrap_or_default();
    assert!(
        sha.is_empty()
            || (matches!(sha.len(), 40 | 64) && sha.bytes().all(|byte| byte.is_ascii_hexdigit())),
        "PACTERN_GIT_SHA must be a full Git commit SHA"
    );
    println!("cargo:rustc-env=PACTERN_BUILD_GIT_SHA={sha}");
}
