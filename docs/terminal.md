# Terminal programs

Pactern programs can read UTF-8 lines, write stdout and stderr, and inspect process
arguments through `host::terminal`. The CLI grants this capability explicitly:

```sh
cd examples/terminal
../../target/debug/pactern run --terminal
../../target/debug/pactern run --terminal -- --help
../../target/debug/pactern run --terminal -- answer
```

All menu text, command parsing, and interaction in this example are Pactern source.
The terminal host connects the program to the three standard streams. Terminal
runs print only the program's output; failures go to stderr. `--json` cannot be
combined with `run --terminal`, since the program owns stdout and stderr.

## Entry point and API

A terminal entry takes exactly one host-issued, owning
`host::terminal::Terminal` parameter and returns `Unit` or an `I64` exit code in
`0..255`. `Unit` exits successfully. For example:

```text
module main;

import host::terminal;

pub fn main(term: host::terminal::Terminal) -> I64
effects { env.read, terminal.write } {
    let count = host::terminal::arg_count(borrow term);
    let args = host::terminal::args(borrow term);
    if (count > 0) {
        host::terminal::write(borrow term, args[0]);
        host::terminal::write(borrow term, "\n");
    }
    return 0;
}
```

| Function | Parameters | Result | Effect |
| --- | --- | --- | --- |
| `host::terminal::arg_count` | `borrow Terminal` | `I64` | `env.read` |
| `host::terminal::args` | `borrow Terminal` | `List<String>` | `env.read` |
| `host::terminal::read_line` | `borrow Terminal` | `Option<String>` | `terminal.read` |
| `host::terminal::write` | `borrow Terminal, String` | `Unit` | `terminal.write` |
| `host::terminal::write_error` | `borrow Terminal, String` | `Unit` | `terminal.write` |

Arguments after the CLI's `--` are passed verbatim, excluding an executable name.
Thus `run --terminal -- --help` passes `--help` to the Pactern program, while
`run --help` displays the toolchain's help. Empty arguments, Unicode, spaces,
and option-looking arguments are preserved. Source paths go before `--`.
The existing `--args JSON` entry-value interface is separate: terminal entries
require its default `[]`, because their sole parameter comes from the host.

`read_line` removes a trailing LF or CRLF and returns `Option::Some(text)`.
An empty line is `Some("")`; EOF without another line is `Option::None`.
A final line without a newline is returned before EOF. Stream errors and invalid
UTF-8 trap. Writes add no newline and flush immediately, allowing prompts without
a trailing newline. Programs add `"\n"` when needed.

## Contracts and limits

Terminal handles obey the same move, borrow, aggregate, and escape restrictions
as quota handles. Calls require declared effects. Contracts remain pure: they
cannot inspect the terminal or perform I/O, but can constrain exit codes and pure
command parsers. Runtime pact guards remain enabled by default; `--no-contracts`
explicitly disables them. Output already written cannot be undone by a later trap.

The terminal ABI is `pactern.terminal.v1`. The pure host continues to reject all
imports, and the terminal host accepts only its versioned functions with exact
signatures. A terminal grant does not grant files, network, or quota storage.
The handle expires with the invocation, including traps and stream failures.

CLI invocations allow at most 1,024 arguments and a cumulative 1 MiB of copied
argument bytes, consumed stdin bytes, and written stdout/stderr bytes. Each input
line allows at most 65,536 UTF-8 bytes, excluding LF/CRLF. CLI terminal builds and
runs allow 10,000,000 Pactern evaluation steps; terminal runs additionally allow
10,000,000,000 Wasm fuel units. These budgets include runtime pact checks and span
the whole session. Pure entries retain their smaller default budgets. Memory,
call depth, and output decoding limits also apply. Input waits synchronously for a
line; guest instruction fuel does not impose a wall-clock deadline while waiting
on the supplied input stream.

`build --terminal` writes a Wasm artifact and generated C for this host, with
artifact-bound guard evidence. Process arguments are supplied when executing the
artifact through `backend::run_wasm_terminal`, rather than embedded at build time.
The embedding API accepts explicit `BufRead`/`Write` streams and a byte budget via
`backend::TerminalIo`. The pure interpreter does not execute terminal I/O.
