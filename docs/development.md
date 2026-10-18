# Development

The repository uses a pinned Rust toolchain. Install or select Rust 1.98.1 with
the `rustfmt` and `clippy` components; the root `rust-toolchain.toml` selects
this toolchain automatically.

The workspace contains three crates:

- `crates/pactern` is the shared Rust library for versioned tool and protocol
  types, syntax APIs, module/name resolution, and core value typing.
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
located diagnostics. The core type checker builds on this resolution result.

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
recorded for later phases; resolution itself does not check types, effects, generic
recursion, handles, or host registries.

Check core types after parsing the closed project:

```rust
use pactern::resolution::ParsedFile;
use pactern::syntax::{parse, FileKind, SourceFile};
use pactern::typing::{check, ValueType};

fn main() -> Result<(), pactern::syntax::SyntaxDiagnostic> {
    let path = "src/example.ptn";
    let source = SourceFile::new(path,
        "module example; fn answer() -> I64 { let n = 42; return n; }");
    let tree = parse(&source, FileKind::Implementation)?;
    let checked = check(&[ParsedFile::new(path, tree)]);
    assert!(checked.is_success());
    assert!(checked.symbol_types.values().any(|ty| *ty == ValueType::I64));
    Ok(())
}
```

`typing::check` performs resolution internally. On resolution failure it returns
those diagnostics in `resolution.diagnostics` and skips typing. Otherwise,
`diagnostics` contains local type errors with file-aware spans and related
declarations where applicable. `symbol_types` and `expression_types` expose fully
inferred ordinary value types; failed or ambiguous entries are omitted. Inspect
`is_success()` before using these partial indexes. Named types and symbols use IDs
from the enclosed resolution, not persistent identities.

Core typing covers `Bool`, checked-`I64` literal ranges, UTF-8 strings, `Unit`,
lists, nominal records/enums, `Option`/`Result`, named calls, local inference,
immutable parameters/bindings, explicit local mutation, returns, loop control,
and Boolean pact clauses. It checks nested match payloads for exhaustive coverage
and unreachable arms, including correlations between payload positions. Infinite
domains, records, lists, and `Unit` require a catch-all pattern. Inference uses the
whole initializer, argument, return, or condition expression; an ambiguous local
needs an annotation before subsequent statements. A loop conservatively needs a
following return in a non-`Unit` function. Well-typed arithmetic and indexing that
would trap at runtime remain valid types.

Expanded types are limited to depth 64 and 1,024 nodes. Each match has a budget of
20,000 analysis units (charging row scans and recursive steps), with recursion
limited to 256. Exceeding these bounds emits `PTN_TYPE_LIMIT` or
`PTN_TYPE_MATCH_LIMIT`; it never counts as successful analysis. Borrowed
parameters/arguments emit `PTN_TYPE_UNSUPPORTED_HANDLE`. Type checking performs no
filesystem or network access and does not execute code. It does not establish
effect bounds, handle safety, contract proofs, or runtime behavior. CLI project
loading and protocol diagnostic envelopes remain later work.

Syntax spans use zero-based UTF-8 byte offsets and one-based line and Unicode
scalar columns. LF, CRLF, and lone CR each count as one newline. Spans are local
to the `SourceFile` that produced them. `SyntaxDiagnostic` reports the first
lexer or parser error with a local span; it is not the revision-bound protocol
diagnostic envelope used for later project operations. The lexer accepts at most
1 MiB of source and 100,000 emitted tokens including EOF. Parser nesting and
expression depth are limited to 64.

The [conformance corpus](../fixtures/conformance/README.md) records expected
Pactern outcomes. Workspace tests check syntax, resolver-phase, and type-phase
diagnostics by staging each fixture's selected files at their declared module
paths; they also exercise the conventional Rust comparison program. They do
not run Pactern programs or check runtime or verification outcomes. With
Go on `PATH`, run both baselines and record their build/execution costs with:

```sh
go test ./baselines/go/main.go ./baselines/go/main_test.go
go vet ./baselines/go/main.go ./baselines/go/main_test.go
cargo run -p pactern-baseline --locked -- --samples 5 --output target/baseline.json
```

See [baseline measurement details](../baselines/README.md) for the task format,
initial results, measured costs, and limitations. Timing is observational and is
not a pass/fail threshold in workspace tests.

### Generic value typing

Rank-1 functions, records, and enums are supported. Each generic body is checked
once with rigid parameters: `fn identity<T>(x: T) -> T { return x; }` works, while
adding integers to `T`, comparing arbitrary `T` values, or returning `T` as another
parameter is rejected even if the function is never called. Each call gets fresh
inference variables; arguments and expected results determine its type arguments.
Explicit calls such as `identity::<I64>(1)` and `Result::Ok::<I64, String>(1)`
check exact type-argument arity. There is no partial explicit specialization.

Record fields and enum payloads infer arguments, including through nested types:
`Box { value: true }` has type `Box<Bool>` for `struct Box<T> { value: T }`.
`let empty: Box<List<I64>> = Box { value: [] };` uses result context. Field access
and match coverage substitute the actual arguments. Nullary variants still omit
parentheses and use annotations for context. Unconstrained arguments, including
unused function parameters such as `fn f<T>() -> Unit {}`, produce
`PTN_TYPE_CANNOT_INFER`; type-argument count errors produce `PTN_TYPE_ARITY`.
Annotations resolve ambiguous locals before later statements, as in core typing.

`ValueType::Applied` exposes a nominal declaration ID and ordered arguments;
`ValueType::Parameter` identifies a rigid parameter in generic declaration bodies.
All IDs belong to the enclosing resolution result. No monomorphized executable is produced. The core lowering phase retains
call-site type arguments in generic IR. Recursive call cycles involving a generic
function (including through nongeneric helpers and changing instantiations) emit
`PTN_TYPE_GENERIC_RECURSION`. Recursive nominal data remains supported within
bounded type and coverage analysis. Handles remain unavailable and cannot be
passed or instantiated as ordinary generic values.


### Typed core IR

`pactern::core::lower(&[ParsedFile])` resolves, checks, and lowers a closed package
without I/O. It returns a `Program` or `LowerError`: `Check` retains resolution and
type diagnostics; `Invariant` identifies a missing compiler annotation or invalid
lowering assumption with a file-aware location. Neither error returns partial IR.
The same parser-produced-input contract and analysis limits as typing apply.

The structured IR contains functions, records, enums, pacts, blocks, statements,
expressions, and patterns. Every expression and pattern carries a `ValueType` and
source origin. Declaration, block, statement, variant, arm, and clause origins
retain file IDs and UTF-8 byte/line/scalar-column spans. Symbol IDs replace local
names, function names, constructors, pact targets, and field names. The program
owns its resolution and symbol-type tables; IDs are local to that program.
Parameter, local, binding, result, and record-field types live in the symbol table.
Function results and enum payload types are explicit in declarations.

Invariants are established by construction: resolution and typing must succeed
(including control flow, mutation rules, coverage and generic recursion checks),
then lowering requires annotations and references for the nodes it emits. There
are no error types or unresolved names in value operations. `Program` has private
storage and shared getters, so callers cannot assemble or mutate a checked
program from arbitrary nodes. This is not an interchange format or a validator
for externally supplied IR. Rust `Debug` provides inspection; JSON serialization
is not part of this milestone.

Generic bodies retain rigid parameter IDs. Calls retain ordered, fully inferred
type arguments, including explicit arguments unused in the signature. Constructor
and record instantiations are represented by their expression's nominal/container
type. Nested pattern payloads have substituted types. No monomorphization occurs.

Consumers must evaluate list items, call/constructor arguments, record fields,
and ordinary binary operands in source order. Record fields deliberately retain
initializer order, not declaration order. `And`/`Or` are short-circuit operations;
match arms remain ordered, and only the selected arm is evaluated. Arithmetic
operators retain checked-I64 semantics and indexes retain bounds checks for the
future interpreter/backend. Parentheses are erased while retaining the outer
expression origin. A directly negated integer becomes a signed `i64`, including
`i64::MIN`; other negation stays an operation. `Return(None)` and normal fallthrough
of a `Unit` function return unit. Break and continue target the enclosing loop.

Effect declarations preserve omission versus an explicit empty bound but are not
validated effect authority. Pacts retain typed Boolean clauses and targets, not
obligation IDs, runtime guards, or verification results. Handles, interpretation,
code generation, and CLI project loading remain later work.

The core tests inspect lowered structure, generic specialization, nested patterns,
field order, UTF-8 origins, imports/contracts, control flow, and rejection paths.
Accepted typing test programs and all successfully typed conformance cases also
pass through lowering; runtime/trap/verification expectations are not executed.
