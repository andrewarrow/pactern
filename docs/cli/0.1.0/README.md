# Pactern CLI 0.1.0

This document describes the milestone 2 command-line scaffold. The CLI can
report version metadata and help. Project commands are reserved and currently
return a diagnostic; they do not read or write project files. The language,
compiler, formatter, verifier, test runner, and change service are not
implemented by this scaffold.

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
| `check` | `pactern check` | Reserved; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `fmt` | `pactern fmt [--check]` | Reserved; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `verify` | `pactern verify` | Reserved; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `test` | `pactern test [--affected]` | Reserved; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `build` | `pactern build [--target wasm]` | Reserved; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `inspect` | `pactern inspect SYMBOL` | Reserved; requires a symbol; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `callers` | `pactern callers SYMBOL` | Reserved; requires a symbol; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `impact` | `pactern impact --change PATH` | Reserved; requires a change path; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |
| `apply` | `pactern apply PATH [--dry-run]` | Reserved; requires a change path; returns `PTN_TOOL_NOT_IMPLEMENTED`, exit 1. |

The `fmt --check` option is reserved and currently exits 1 with the same
not-implemented diagnostic as formatting. The `build` target currently accepts
only `wasm`; another target is a bad invocation and exits 2. No command currently
opens the supplied project or change-request paths.

## Exit status

- `0`: version or help output was written successfully.
- `1`: a well-formed project command is unavailable, or output failed.
- `2`: the invocation is invalid or the selected protocol version is
  unsupported.

