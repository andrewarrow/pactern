# Candidate build and editor integration

The candidate version is 0.1.0. Install from a complete source checkout with the
pinned Rust 1.98.1 toolchain and Cargo.lock:

```sh
cargo install --locked --path crates/pactern-cli
pactern --json version
```

This is a local installable source candidate; it is not a crates.io release.
The shared library embeds the C runtime from the workspace, so retain the complete
checkout when building. Clang 21.1.8, wasm-ld 21.1.8 and Z3 5.3.1 are required for
the full compiled/verification suite. Go 1.27.1 runs the conventional baselines.
External tool versions are recorded in evidence; the CI gate rejects drift from
these versions. Homebrew installation is not a vendored, hermetic toolchain.

Create `src/main.ptn` in a fresh project:

```pactern
module main;
fn answer() -> I64 {
    return 42;
}
```

Then run the same examples exercised by the candidate script:

```sh
pactern --json check src/main.ptn
pactern --json fmt --check src/main.ptn
```

Add `contracts/main.pact.ptn`:

```pactern
module main;
pact answer_is_42 for answer {
    ensures result == 42;
}
```

Verify the contract, build an invocation artifact, and execute it:

```sh
pactern --json verify src/main.ptn contracts/main.pact.ptn
pactern --json build --entry main::answer --output answer.wasm src/main.ptn contracts/main.pact.ptn
pactern --json run --entry main::answer src/main.ptn contracts/main.pact.ptn
```

`./scripts/validate-candidate.sh` copies a clean source snapshot excluding `.git`
and `target`, uses a fresh target and install directory, checks
formatting and all Clippy targets, runs the full Rust and Go suites, builds the
release workspace, installs the CLI, compares all version fields with the
workspace version, and runs these documentation examples using the installed
binary. It fails on missing tools or failed tests.

Local validation on 2026-10-18 passed on macOS arm64: formatting, Clippy with
warnings denied, 205 Rust tests (zero failed or ignored), the Go tests and vet,
the release build, installation, and all installed documentation examples above.
The separate protocol validator passed 202 instance checks across 10 schemas,
including actual CLI/editor output and freshly generated workflow evidence.
Reproduce those checks with the [development commands](development.md).
These results describe local execution; remote CI activation is separate.

`pactern::editor` supplies revision-bound diagnostics and navigation
from the shared semantic APIs. It is an editor adapter API, not a complete LSP
server or packaged editor extension. See its tests for exact UTF-8/UTF-16 position
and revision behavior.

The candidate workflow has a separate acceptance job that checks out the PR base,
builds the acceptance executable from that trusted checkout, and examines the
candidate without executing its gate code. A baseline without the gate fails:
the first trusted bootstrap and required branch-protection configuration must be
approved outside this candidate. This repository cannot activate those server
settings by changing its own workflow. Protected baseline changes remain an
external review step. A successful local full-suite run does not establish that
remote CI or branch protection has been activated.
