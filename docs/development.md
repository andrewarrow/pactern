# Development

The repository uses a pinned Rust toolchain. Install or select Rust 1.98.1 with
the `rustfmt` and `clippy` components; the root `rust-toolchain.toml` selects
this toolchain automatically.

The workspace contains three crates:

- `crates/pactern` is the shared Rust library for versioned tool and protocol
  types and, as implementation proceeds, compiler and semantic APIs.
- `crates/pactern-cli` is the `pactern` binary. It adapts command-line
  arguments and terminal output to the shared library.
- `crates/pactern-baseline` validates recorded conformance fixtures and runs
  the Go/Rust seed comparisons. It is a development tool, separate from the CLI.

The CLI uses Clap for argument parsing. Serde and serde_json provide structured
serialization. Keep `Cargo.lock` tracked so workspace and installed CLI builds
use the reviewed dependency resolution.

From the repository root, build and check the workspace with:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Install the CLI from this checkout with:

```sh
cargo install --path crates/pactern-cli --locked
```

Run the version command directly from the workspace with:

```sh
cargo run -p pactern-cli -- version --json
```

At this milestone, version and help output work. Compiler and project commands
are still scaffolded and return a not-implemented diagnostic.

The [conformance corpus](../fixtures/conformance/README.md) records expected
Pactern outcomes without claiming execution. Workspace tests validate its
metadata and exercise the conventional Rust comparison program. With Go on
`PATH`, run both baselines and record their build/execution costs with:

```sh
go test ./baselines/go/main.go ./baselines/go/main_test.go
go vet ./baselines/go/main.go ./baselines/go/main_test.go
cargo run -p pactern-baseline --locked -- --samples 5 --output target/baseline.json
```

See [baseline measurement details](../baselines/README.md) for the task format,
initial results, measured costs, and limitations. Timing is observational and is
not a pass/fail threshold in workspace tests.
