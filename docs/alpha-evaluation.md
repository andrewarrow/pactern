# Candidate evaluation: narrow the alpha

The decision is **narrow**: continue with the checked pure fragment, protected
semantic edits, and the quota workflow as a developer preview. There is enough
executed functionality to evaluate the design. There is not enough experimental
evidence to claim an agent-productivity advantage or a general-purpose alpha.

The actual runs are [Go/Rust](../baselines/results/candidate-go-rust.json) and
[Pactern](../baselines/results/candidate-pactern.json). Each measures the same 25
public seed cases selected by `baselines/tasks.json`: quota, adjacent-pair totals,
and labelled capacity summaries. Every measured process returns results checked
against the corpus (Go/Rust) or the corresponding reference interpreter mode
(Pactern). One untimed warmup precedes five fresh-process samples per mode. These
are 125 checked case executions per mode, plus warmup; they are not 125 independent
repair trials.

| Implementation | Runtime contracts | Batch median (ms) | Sample standard deviation (ms) | Samples | Artifact bytes |
| --- | --- | ---: | ---: | ---: | ---: |
| Rust native | enabled | 1.811 | 0.202 | 5 | 582,272 |
| Go native | enabled | 2.739 | 0.219 | 5 | 3,767,506 |
| Pactern / Wasmi | disabled | 5.048 | 0.081 | 5 | 183,687 |
| Pactern / Wasmi | enabled | 5.806 | 0.095 | 5 | 262,467 |

These measurements describe startup and batch execution costs under different
runtime boundaries. Rust and Go read JSON requests and execute a native program.
Pactern reads JSON descriptors and 25 input-specialized artifacts, validates and
translates each module, instantiates a fresh Wasmi store, then executes it. Its
artifact column sums all 25 Wasm modules and excludes the shared Rust host binary;
the native rows count their complete executables. C/Wasm compilation is outside
the timed interval. Therefore neither times nor sizes establish a language
performance ranking. The fixed input artifacts can also enable constant folding;
this is a limitation of the present invocation ABI.

The Pactern pair is a real runtime contract ablation: both modes compile and run
all 25 inputs. The unguarded negative-quota input intentionally returns a value
instead of enforcing the precondition. Each mode is checked against its own
reference behavior; removing a guard is not counted as satisfying that guard.
The Go/Rust harness's legacy `pactern_execution: not_implemented` field describes
that older harness only. Pactern execution is recorded in the separate report.

Reports preserve raw nanosecond samples, build scopes, tool versions, outcomes,
source snapshots or hashes, and generated artifact hashes. The Pactern report
also records sample variance. The machine was macOS arm64 with Rust 1.98.1,
Go 1.27.1, Clang/LLD 21.1.8 and Wasmi 2.0.0. CPU pinning, background load, thermal
state, OS caches and compiler downloads were not controlled. Five samples cannot
support stable tail-latency or population estimates. Peak RSS, p99, sustained
throughput, and hosted cost remain unmeasured.

Reproduce the reports from a source checkout:

```sh
cargo run -p pactern-baseline --locked -- --samples 5 --output target/candidate-go-rust.json
cargo run -p pactern --release --locked --example runtime_benchmark -- target/candidate-pactern.json
```

The [continuation targets](../PLAN.md#10-define-better-before-claiming-it) remain
open. No matched-model agent repair experiment has run. Token/currency cost,
30% token reduction, 50% repair-cycle reduction, review accuracy/time, and
text-versus-semantic-edit ablations are **unmeasured**. Baseline-success parity on
held-out repair tasks is **unestablished**. Runtime import-denial, transaction
isolation, protected-edit, and replay tests exercise the implemented boundaries;
they do not prove universal absence of escapes.

A broader alpha decision requires held-out changes with fixed model versions,
budgets, documentation and tool-cost accounting; text/semantic and contract
ablations for each applicable language; all failures retained; and a review study.
The current result supports narrowing the application scope while collecting that
evidence. It does not support asserting that milestones 29–30 have satisfied every
original release and evaluation criterion. In particular remote CI/branch
protection needs an external trusted bootstrap, and the toolchain is version
checked rather than hermetically vendored.
