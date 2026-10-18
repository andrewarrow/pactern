#!/bin/sh
# Full local release gate. Tools are installed by the caller/CI, never implicitly.
set -eu
candidate_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$candidate_root"
PACTERN_GIT_SHA=${PACTERN_GIT_SHA:-$(git rev-parse --verify HEAD 2>/dev/null || true)}
export PACTERN_GIT_SHA
release_scratch=$(mktemp -d "${TMPDIR:-/tmp}/pactern-candidate.XXXXXX")
trap 'rm -rf "$release_scratch"' EXIT HUP INT TERM
mkdir -p "$release_scratch/source"
tar --exclude='./target' --exclude='./.git' -cf "$release_scratch/source.tar" .
tar -C "$release_scratch/source" -xf "$release_scratch/source.tar"
cd "$release_scratch/source"
export CARGO_TARGET_DIR="$release_scratch/target"

rustc --version | python3 -c 'import sys; v=sys.stdin.read(); assert v.startswith("rustc 1.98.1 "), v'
# Locked Rust dependencies and toolchain; no check is skipped if a prerequisite is absent.
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
go test ./baselines/go/main.go ./baselines/go/main_test.go
go vet ./baselines/go/main.go ./baselines/go/main_test.go
cargo install --locked --path crates/pactern-cli --root "$release_scratch/install" --target-dir "$CARGO_TARGET_DIR"

candidate_bin="$release_scratch/install/bin/pactern"
"$candidate_bin" --json version > "$release_scratch/version.json"
python3 - "$release_scratch/version.json" <<'PY'
import json, os, sys, tomllib
with open(sys.argv[1]) as f:
    version = json.load(f)
with open('Cargo.toml', 'rb') as f:
    expected = tomllib.load(f)['workspace']['package']['version']
assert version['tool_version'] == expected, version
assert version['git_sha'] == (os.environ['PACTERN_GIT_SHA'] or None), version
assert version['cli_version'] == version['language_version'] == version['protocol_version'] == expected, version
PY
# These commands are the runnable examples in docs/candidate.md.
mkdir -p "$release_scratch/project/src" "$release_scratch/project/contracts"
cat > "$release_scratch/project/src/main.ptn" <<'PTN'
module main;
fn answer() -> I64 {
    return 42;
}
PTN
cat > "$release_scratch/project/contracts/main.pact.ptn" <<'PTN'
module main;
pact answer_is_42 for answer {
    ensures result == 42;
}
PTN
(cd "$release_scratch/project" && "$candidate_bin" --json check src/main.ptn)
(cd "$release_scratch/project" && "$candidate_bin" --json fmt --check src/main.ptn)
(cd "$release_scratch/project" && "$candidate_bin" --json verify src/main.ptn contracts/main.pact.ptn)
(cd "$release_scratch/project" && "$candidate_bin" --json build --entry main::answer --output "$release_scratch/answer.wasm" src/main.ptn contracts/main.pact.ptn)
(cd "$release_scratch/project" && "$candidate_bin" --json run --entry main::answer src/main.ptn contracts/main.pact.ptn > "$release_scratch/run.json")
python3 - "$release_scratch/run.json" "$release_scratch/answer.wasm" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    result = json.load(f)
assert result['outcome']['value'] == '42', result
with open(sys.argv[2], 'rb') as f:
    assert f.read(8) == b'\0asm\x01\0\0\0'
PY
printf '%s\n' "Candidate validation passed: clean locked build, full checks, installation, and version-matched documentation examples."
