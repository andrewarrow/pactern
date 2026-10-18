# Pactern CLI 0.1.0

The experimental CLI checks and formats source, verifies a narrow pure fragment,
compiles and executes guarded Wasm, runs seeded properties, and exposes semantic
queries and managed atomic changes. `pactern help COMMAND` describes each command.

## Output and options

`--json` selects one JSON document on stdout, including failures. Errors use the
[diagnostic protocol](../../spec/0.1.0/schemas/diagnostic.schema.json), with original
UTF-8 byte spans and related declarations when available. Without `--json`, errors
go to stderr; project reports are pretty-printed JSON. Editor diagnostics use
zero-based UTF-16 columns. JSON I64 values are exact decimal strings.

`run --terminal` explicitly grants stdin/stdout/stderr and process arguments to a
single `host::terminal::Terminal` entry parameter. It prints the program's output
directly, without an execution report, and returns its `I64` exit code (0..255) or
zero for `Unit`. Pass program arguments after `--`, for example
`pactern run --terminal -- --help`. `--json` is incompatible with terminal runs.
`build --terminal` creates artifacts for the terminal host. Runtime pact guards
remain enabled. See [terminal programs](../../terminal.md) for API and limits.

Global flags work before the `--` terminator:

- `--protocol-version 0.1.0` requires an exact protocol version.
- `--help`, `-h`, or `help [COMMAND]` prints help.
- `--version`, `-V`, or `version` prints matched tool/CLI/language/protocol versions.
- `--store PATH` selects an atomic managed workspace.
- `--baseline PATH` selects the trusted owner's external acceptance baseline.

Existing [version](version.schema.json), [help](help.schema.json), and
[source success](source.schema.json) schemas remain valid. Runtime/proof/test/build
reports are experimental; do not treat local numeric compiler IDs as persistent
semantic IDs.

## Commands

| Command | Behavior |
| --- | --- |
| `check [PATH ...]` | Checks syntax, names, values, effects, and handle ownership. |
| `fmt [--check] [PATH ...]` | Preserves comments/tokens and applies canonical layout. `--check` never writes. |
| `verify [PATH ...]` | Runs external Z3 for every pact target, with separate consistency, postcondition, trap, and termination claims. |
| `build [PATH ...] --entry NAME --args JSON [-o FILE]` | Writes guarded invocation-specialized Wasm and generated C; default output is `target/pactern.wasm`. |
| `run [PATH ...] --entry NAME --args JSON` | Compiles and runs in the bounded deny-import host. Optional `-o FILE` retains artifacts. |
| `test [PATH ...] --entry NAME [--seed N] [--cases N]` | Compares guarded interpreter and Wasm executions for seeded I64/Bool properties. |
| `inspect SYMBOL` | Returns a declaration with source, signature, hash, dependencies, and identity. |
| `callers SYMBOL` | Returns referencing functions with completeness information. |
| `impact --change FILE` | Checks the request revision and returns conservatively affected declarations/interfaces/pacts. |
| `editor` | Emits editor diagnostics over the selected workspace. |
| `editor --path FILE --byte OFFSET` | Looks up the definition at a UTF-8 byte offset. |
| `init [PATH ...] --store PATH --baseline PATH` | Imports a source package and creates a new external baseline and managed store. |
| `apply FILE [--dry-run] --store PATH --baseline PATH` | Stages a revision-checked request, enforces protection and required proof, and atomically publishes the managed snapshot. |

A unique function or `main` can be selected implicitly. Entry points must be
nongeneric; generic helpers work. `--args` defaults to `[]`. Bool, strings, lists,
records, tagged enums, Option and Result are decoded according to the entry's
types. I64 JSON numbers are rejected to avoid precision loss. `build`/`run` accept
only `--target wasm`; `--no-contracts` selects the explicit unguarded ablation.

`verify` exits nonzero for refutation, unknown, unsupported, inconsistent premises,
or no pacts. A timeout is not a successful proof. `test` defaults to seed 1 and
20 cases, with a maximum of 10,000. It discards false preconditions and records
seeds and failures; it does not establish unbounded validity or trap freedom.
`test --affected` currently runs the complete selected-entry property suite,
reporting that dependency filtering is unavailable. The semantic library's
`affected_tests` API supports explicit test dependency inventories.

## Source selection and limits

Without paths, source commands select `.ptn` files under `src` and `contracts` in
the current directory. Explicit paths replace that selection. The resolver uses
`src/<module>.ptn` and `contracts/<module>.pact.ptn`; imported modules must be
included. Imports do not cause filesystem or network reads. The registered quota
and terminal host namespaces are supplied by the compiler.

Paths stay inside the current directory. Symbolic links and overlapping selections
are rejected. Limits are 256 files, 16 MiB total, directory depth 64, and 1 MiB per
file. Every implementation and contract is also limited to **1,000 physical lines**,
including blank lines and comments. CRLF, LF, and CR count once; a trailing newline
does not add an empty line. `PTN_LEX_LINE_LIMIT` points at the first excess line.

Formatting preserves exact literals, token spellings, comments, parentheses, and
trailing commas. Layout uses four spaces and LF; comment bytes are preserved.
It is idempotent. Output beyond the byte or line budget receives
`PTN_TOOL_FORMAT_LIMIT` or `PTN_TOOL_FORMAT_LINE_LIMIT` before any selected file is
written. `fmt --check` reports `PTN_TOOL_FORMAT_REQUIRED` when layout differs.
Individual formatting writes use atomic replacement and preserve permissions;
a multi-file format is not a transaction. `check` and `fmt` operate on ordinary
source files, even if managed-workspace flags are present.

The source report's `source_revision` hashes the input bytes, paths, kinds, and
frontend profile; it differs from the full managed revision binding identities,
external inputs, and policy. Successful static checking does not imply proof.

## Managed changes and trust

Choose an existing parent directory outside the candidate tree for the baseline,
and a new managed-store directory. Store paths must not contain symlinks; on macOS,
use canonical paths rather than `/tmp` or `/var` aliases.

```sh
pactern init --store ../managed-project --baseline ../project-baseline.json --json
pactern inspect quota::reserve --store ../managed-project \
  --baseline ../project-baseline.json --json
pactern apply repair.json --dry-run --store ../managed-project \
  --baseline ../project-baseline.json --json
pactern apply repair.json --store ../managed-project \
  --baseline ../project-baseline.json --json
```

`init` refuses existing stores/baselines, protects pacts and their helper closure,
and requires frontend checking plus actual unbounded verification for pact targets
on subsequent edits. A required unsupported proof blocks an edit. Protecting a
pact cannot make an implementation correct without the required evidence.
Specification changes require externally issued exact approvals through the
library/service API; the CLI does not invent an approval flag.

`apply` uses the [semantic-change schema](../../spec/0.1.0/schemas/change-request.schema.json).
The managed store atomically publishes all source bytes and identities in one
snapshot. Stale, invalid, unproved, or protected changes leave it unchanged. This
operation does not overwrite ordinary editor-owned source trees. A managed source
snapshot can be used directly by build/run/verify/test and semantic queries.
Without a stored ledger, source-only query IDs are provisional and scoped to the
source revision; managed identities persist across supported renames/moves.

The CLI is a client, not the security boundary against a malicious local operator.
CI must fix the baseline, acceptance executable, checks, and approval channel
outside candidate control. See [semantic trust](../../semantic.md) and
[candidate CI activation](../../candidate.md).

## Executable example

After `cargo build --workspace --locked`, run from the checkout:

```sh
(cd fixtures/conformance && ../../target/debug/pactern verify \
  src/quota.ptn contracts/quota.pact.ptn --json)
(cd fixtures/conformance && ../../target/debug/pactern run \
  --entry quota::reserve --args '["3","2"]' \
  src/quota.ptn contracts/quota.pact.ptn --json)
cargo run -p pactern --example quota_workflow --locked -- target/quota-workflow
```

## Exit codes

- `0`: the requested operation completed; editor diagnostics can still contain errors.
- `1`: source/contract/runtime/check failure, unavailable external tool, stale or
  protected edit, unsupported/unknown proof, or I/O/resource failure.
- `2`: invalid CLI arguments or unsupported protocol version.
