# Development

The repository uses a pinned Rust toolchain. Install or select Rust 1.98.1 with
the `rustfmt` and `clippy` components; the root `rust-toolchain.toml` selects
this toolchain automatically.

The workspace contains three crates:

- `crates/pactern` is the shared Rust library for versioned tool and protocol
  types and the syntax-only source, lexer, and parser APIs.
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

The CLI's version and help output work. Its project commands remain scaffolded
and return a not-implemented diagnostic. The library parser accepts implementation
and contract source files, but it does not resolve names or check types.

Use the syntax API directly when testing grammar behavior:

```rust
use pactern::syntax::{parse, FileKind, SourceFile};

fn main() -> Result<(), pactern::syntax::SyntaxDiagnostic> {
    let source = SourceFile::new(
        "src/example.ptn",
        "module example; pub fn answer() -> I64 { return 42; }",
    );
    let tree = parse(&source, FileKind::Implementation)?;
    assert_eq!(tree.declarations.len(), 1);
    Ok(())
}
```

Syntax spans use zero-based UTF-8 byte offsets and one-based line and Unicode
scalar columns. LF, CRLF, and lone CR each count as one newline. Spans are local
to the `SourceFile` that produced them. `SyntaxDiagnostic` reports the first
lexer or parser error with a local span; it is not the revision-bound protocol
diagnostic envelope used for later project operations. The lexer accepts at most
1 MiB of source and 100,000 emitted tokens including EOF. Parser nesting and
expression depth are limited to 64.

The [conformance corpus](../fixtures/conformance/README.md) records expected
Pactern outcomes; workspace tests parse every corpus source and exercise the
conventional Rust comparison program, but do not execute Pactern programs.
With Go on `PATH`, run both baselines and record their build/execution costs
with:

```sh
go test ./baselines/go/main.go ./baselines/go/main_test.go
go vet ./baselines/go/main.go ./baselines/go/main_test.go
cargo run -p pactern-baseline --locked -- --samples 5 --output target/baseline.json
```

See [baseline measurement details](../baselines/README.md) for the task format,
initial results, measured costs, and limitations. Timing is observational and is
not a pass/fail threshold in workspace tests.
