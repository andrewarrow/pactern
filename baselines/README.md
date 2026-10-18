# Go/Rust seed baselines

These are executable comparison programs for milestone 3, before Pactern has an
interpreter or backend. They implement three public seed tasks independently of
the expected results: quota reservation, adjacent-pair totals, and a labelled
capacity summary. They use conventional Rust and Go, with explicit overflow and
bounds checks where Pactern requires traps. Both execute the quota precondition
and postcondition, including postconditions on recoverable `Err` results.

From the repository root, with the pinned Rust toolchain and Go installed:

```sh
cargo test --workspace --locked
go test ./baselines/go/main.go ./baselines/go/main_test.go
go vet ./baselines/go/main.go ./baselines/go/main_test.go
cargo run -p pactern-baseline --locked -- --samples 5 --output target/baseline.json
```

The harness needs `cargo`, `rustc`, and `go` on `PATH`. The initial recorded run
used Rust 1.98.1 and Go 1.27.1 on macOS arm64. No new Rust dependencies or Go
modules are required. Running Rust workspace tests does not require Go; the last
command builds and exercises both languages. This is a development harness run
from a source checkout, separate from the installable `pactern` CLI.

[`tasks.json`](tasks.json) selects 25 cases by stable ID from the
[conformance manifest](../fixtures/conformance/manifest.json). Inputs and expected
results live in that corpus, avoiding a second copy of the oracle. Each program
receives an array of `{id, task, args}` requests on stdin and writes ordered
`{id, outcome, pacts?}` responses on stdout. It never receives expected results.
Integers travel as exact decimal strings; lists and records use JSON aggregates.
`Err` is a returned value; a trap is a distinct outcome. The harness validates
versions, paths, task references, argument shapes and integer ranges, then rejects
missing, extra, reordered, or differing responses, including wrong pact states.
Any build, process, decoding, or comparison failure exits nonzero without writing
a new successful report.

The [initial report](results/initial.json) records one untimed warmup followed by
five checked samples per language. Every sample launches a fresh process for the
entire 25-case batch. Times include process startup, stdin/stdout, JSON handling,
and computation, but exclude the harness's response comparison. They are not
individual function latencies or throughput measurements. Nanosecond durations
are integer numbers. The median is the lower middle sample; p95 uses nearest
rank, so with five samples it is the maximum. Preserve the raw samples when
comparing runs, and use more samples for any statistical claim.

Build wall time and executable bytes are recorded separately. Each run uses a
fresh Rust target directory and Go build cache, removed on exit. Rust builds
include dependencies and Go builds include the standard library. Downloads,
toolchain/linker and OS caches, machine load, and build configuration outside the
repository remain uncontrolled. Runtime order is Rust then Go, without CPU
pinning. These costs establish a reproducible smoke baseline, not a performance
ranking. Reports snapshot the exact fixture, task, comparison program, harness,
and manifest/lockfile text alongside tool versions and platform information.

The other recorded conformance cases are metadata-validated only. Static errors,
Pactern runtime behavior, and verification expectations await their respective
compiler milestones. The `pactern_execution` field remains `not_implemented`.

No agent repair experiment has run: model versions, prompts, token/currency
costs, repair success, review effort, peak memory, and hosted runtime costs are
unmeasured. These seed cases are public, not held out. Later evaluation must use
separate held-out tasks, matched models/budgets/documentation, text/semantic-edit
and contract ablations, and unsuccessful runs as described in
[the evaluation plan](../PLAN.md#10-define-better-before-claiming-it).
