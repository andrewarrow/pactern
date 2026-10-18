#!/bin/sh
# Run this script FROM THE TRUSTED BASELINE CHECKOUT. Never source a candidate copy.
set -eu
if [ "$#" -ne 2 ]; then
    echo "usage: check-protected-baseline.sh TRUSTED_BASELINE CANDIDATE" >&2
    exit 2
fi
baseline_root=$(cd "$1" && pwd -P)
candidate_root=$(cd "$2" && pwd -P)
if [ ! -f "$baseline_root/crates/pactern/examples/acceptance_gate.rs" ]; then
    echo "The pinned baseline lacks the acceptance gate; trusted bootstrap is required." >&2
    exit 2
fi
gate_build=$(mktemp -d)
trap 'rm -rf "$gate_build"' EXIT HUP INT TERM
CARGO_TARGET_DIR="$gate_build" cargo build --locked --manifest-path "$baseline_root/Cargo.toml" -p pactern --example acceptance_gate
"$gate_build/debug/examples/acceptance_gate" "$baseline_root" "$candidate_root"
