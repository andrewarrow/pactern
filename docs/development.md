# Development

The repository uses a pinned Rust toolchain. Install or select Rust 1.98.1 with
the `rustfmt` and `clippy` components; the root `rust-toolchain.toml` selects
this toolchain automatically.

The workspace contains three crates:

- `crates/pactern` is the shared Rust library for versioned tool and protocol
  types, syntax APIs, and module/name resolution.
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
and return a not-implemented diagnostic. The library parses implementation and
contract source files and resolves parsed files together as a project. Resolution
uses package-relative module paths and returns symbol/reference indexes and
located diagnostics; it does not check types.

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

Resolve parsed files together to build a package-local symbol and reference
index:

```rust
use pactern::resolution::{resolve, ParsedFile};
use pactern::syntax::{parse, FileKind, SourceFile};

fn main() -> Result<(), pactern::syntax::SyntaxDiagnostic> {
    let path = "src/example.ptn";
    let source = SourceFile::new(
        path,
        "module example; pub fn answer() -> I64 { return 42; }",
    );
    let tree = parse(&source, FileKind::Implementation)?;
    let project = resolve(&[ParsedFile::new(path, tree)]);
    assert!(project.is_success());
    Ok(())
}
```

Pass every implementation and contract file in the closed project using its
package-relative path (for example, `src/util/math.ptn` or
`contracts/util/math.pact.ptn`). Resolution performs no filesystem or network
access. Its file, module, and symbol IDs belong to one result and are not
persistent identities. Diagnostics and references include a file ID because
syntax spans are local to one source file. Field accesses and effect labels are
recorded for later phases; resolution does not check types, effects, generic
recursion, handles, or host registries.

Syntax spans use zero-based UTF-8 byte offsets and one-based line and Unicode
scalar columns. LF, CRLF, and lone CR each count as one newline. Spans are local
to the `SourceFile` that produced them. `SyntaxDiagnostic` reports the first
lexer or parser error with a local span; it is not the revision-bound protocol
diagnostic envelope used for later project operations. The lexer accepts at most
1 MiB of source and 100,000 emitted tokens including EOF. Parser nesting and
expression depth are limited to 64.

The [conformance corpus](../fixtures/conformance/README.md) records expected
Pactern outcomes. Workspace tests check syntax outcomes and resolver-phase
diagnostics by staging each fixture's selected files at their declared module
paths; they also exercise the conventional Rust comparison program. They do
not run Pactern programs or check type, runtime, or verification outcomes. With
Go on `PATH`, run both baselines and record their build/execution costs with:

```sh
go test ./baselines/go/main.go ./baselines/go/main_test.go
go vet ./baselines/go/main.go ./baselines/go/main_test.go
cargo run -p pactern-baseline --locked -- --samples 5 --output target/baseline.json
```

See [baseline measurement details](../baselines/README.md) for the task format,
initial results, measured costs, and limitations. Timing is observational and is
not a pass/fail threshold in workspace tests.
