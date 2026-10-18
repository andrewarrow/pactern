# Pactern CLI 0.1.0

The CLI reports version metadata and help, checks core value types in source-only
packages, and formats source. Verification, execution, builds, semantic queries,
and change requests remain reserved commands.

## Output

`pactern --json` selects JSON output globally, including for errors, when it
appears before the `--` argument terminator. Each successful metadata or help
response is one JSON object followed by a newline on stdout. Errors are emitted
as one protocol 0.1.0 diagnostic object followed by a newline on stdout. The
diagnostic uses the existing [protocol schema](../../spec/0.1.0/schemas/diagnostic.schema.json);
this CLI document does not define another diagnostic format.

Without `--json`, help and version information go to stdout, while errors go to
stderr. The stderr stream is normally empty for successful commands. A failure
to write output exits with status 1; the CLI makes a best-effort text error
report on stderr.

Version and help responses have their own output contracts. Both carry
`cli_version: "0.1.0"`, independently of the protocol version. The language
version in version output identifies the target language specification; it does
not claim that language conformance has been implemented.

- [Version output schema](version.schema.json)
- [Help output schema](help.schema.json)
- [Source command success schema](source.schema.json)

Each `check` or `fmt` invocation also emits exactly one JSON object. On failure,
the CLI reports the first diagnostic in deterministic file/analysis order; the
library snapshot API exposes every diagnostic from the failing frontend stage.
Located errors preserve the compiler's stable codes, phase, UTF-8 byte spans, and
related declaration spans. Plain text prints paths and byte ranges on stderr.

The JSON help `text` is human-readable terminal help, not a machine-readable
command catalog. Use the CLI parser and help text for interactive guidance; the
schema guarantees only the response envelope.

## Global options

- `--json` selects JSON output.
- `--protocol-version VERSION` selects the exact diagnostic protocol version.
  Only `0.1.0` is supported. An unknown version is rejected with a diagnostic
  in protocol 0.1.0 and exit status 2.
- `-h`, `--help` prints top-level help; `--help` can also be used with a
  command. `help [COMMAND]` prints top-level or command help.
- `-V`, `--version`, or the `version` command prints version metadata.

Global options are recognized before `--`. After the terminator, arguments are
passed to the selected command as positional values.

## Commands

| Command | Syntax | Scaffold behavior |
| --- | --- | --- |
| `version` | `pactern version` | Prints version metadata; exits 0. |
| `check` | `pactern check [PATH ...]` | Checks syntax, names, and core value types; exits 0 on success, 1 on a source error. |
| `fmt` | `pactern fmt [--check] [PATH ...]` | Formats source; `--check` exits 1 if formatting is needed and never writes. |
| `verify` | `pactern verify` | Reserved; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `test` | `pactern test [--affected]` | Reserved; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `build` | `pactern build [--target wasm]` | Reserved; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `inspect` | `pactern inspect SYMBOL` | Reserved; requires a symbol; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `callers` | `pactern callers SYMBOL` | Reserved; requires a symbol; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `impact` | `pactern impact --change PATH` | Reserved; requires a change path; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `apply` | `pactern apply PATH [--dry-run]` | Reserved; requires a change path; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |

The `build` target currently accepts only `wasm`; another target is a bad
invocation and exits 2. Reserved commands never open project/change-request paths.

## Source selection and formatting

Without paths, `check` and `fmt` recursively select `.ptn` files in `src` and
`contracts` under the current directory. Explicit files/directories replace that
selection. `*.pact.ptn` files use the contract grammar; other `.ptn` files use the
implementation grammar. Paths must remain inside the current directory, be UTF-8,
and normalize to the protocol's workspace-relative path syntax. Symbolic links and
overlapping selections are rejected. Selection is bounded to 256 files, 16 MiB in
total, directory depth 64, and the lexer's 1 MiB per-file limit. Empty selections
return `PTN_TOOL_NO_SOURCES`; exceeded loading limits return `PTN_TOOL_SOURCE_LIMIT`.

`check` treats the selection as a closed package: imported modules must be among
the selected files, with the resolver's `src/<module>.ptn` and
`contracts/<module>.pact.ptn` paths. No imports trigger additional reads or downloads.
Success establishes core value typing, including generic and Boolean pact-clause
typing; it does not establish effects, handles, runtime guards, or verification.
There is no manifest, dependency, policy, or identity-ledger loader yet.

`fmt` checks syntax without requiring successful resolution or typing. It preserves
exact token spellings, comments, string escapes, parentheses, and trailing commas.
Layout uses four-space indentation and LF, leaving comment contents verbatim.
Repeated formatting produces identical bytes. Output expansion beyond 1 MiB
returns `PTN_TOOL_FORMAT_LIMIT`. `fmt --check` reports `PTN_TOOL_FORMAT_REQUIRED`
with a source span for the first file needing formatting.

Formatting validates every selected file before writing any. Changed files are
replaced individually through temporary files in the same directory, preserving
permissions. A multi-file format is not a transaction: a later I/O failure can
leave earlier files formatted. Concurrent edits require rerunning the command;
this formatter does not implement the protected change service of milestone 23.

Source success objects contain `kind: "check"` or `"format"`, `check_only`, and
sorted `files` entries with `path` and `changed`. Their `source_revision` identifies
the **input** bytes, including when formatting writes new bytes. Read-only checks
always have `changed: false`.

The diagnostic/source-command revision uses protocol H framing over selected file
bytes, their kinds, and the source-only frontend profile described in the
[development guide](../../development.md#formatting-and-source-diagnostics).
It binds offsets to that source snapshot; it is not the full trusted workspace
revision needed for future acceptance, evidence, or semantic edits. Persistent
symbol IDs are not allocated yet: protocol `symbol_id` is omitted, and temporary
resolver IDs are never serialized as persistent identities.

## Exit status

- `0`: the requested metadata, check, or format operation succeeded.
- `1`: source checking failed, formatting is needed, loading/I/O failed, a
  limit was exceeded, or a well-formed reserved command is unavailable.
- `2`: the invocation is invalid or the selected protocol version is
  unsupported.

