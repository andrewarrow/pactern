# Pactern

> Agents write. You set the terms.

Pactern is a programming language and toolchain being built for developers who
want to delegate implementation while retaining control of what their software
must do, what it may access, and what evidence is required to accept a change.

An agent can fix a failing test by correcting the implementation. It can also
change the expected result, remove a check, or relax a requirement. After that
edit, a green test suite may describe a different agreement from the one you
started with. Reviewing the code means reviewing the rules again, too.

Pactern's answer is to make that agreement a first-class part of the toolchain.
You define the behavior and permissions. The agent proposes an implementation.
The compiler, acceptance tools, and runtime are designed to check that the change
respects the agreement—and return useful evidence when it doesn't.

**Developer preview:** the Rust frontend checks values, declared effects, and
scoped handle ownership. The pure interpreter and generated C/WebAssembly backend
execute the conformance corpus with optional pact guards. Narrow SMT verification,
artifact-bound evidence, persistent semantic identities, managed atomic edits,
and a transactional quota/replay host are available. Pactern source and contract
files have a hard 1,000-line limit. See the [candidate guide](docs/candidate.md),
[implementation status](MILESTONES.md), and [alpha evaluation](docs/alpha-evaluation.md)
for tested behavior and remaining release gates.

## Quickstart

You need Git, Rust installed through `rustup`, and Clang 21 plus `wasm-ld` 21
to compile Pactern programs. The checkout's `rust-toolchain.toml` selects Rust
1.98.1 automatically. On macOS with Homebrew, install the Wasm build tools with:

```sh
brew install llvm@21 lld@21
```

Pactern discovers those Homebrew tools automatically. On other systems, put
`clang` and `wasm-ld` on `PATH`, or set `PACTERN_CLANG` and `PACTERN_WASM_LD`
to their executable paths. No WASI SDK or target sysroot is needed. See the
[backend guide](docs/backend.md#build-tools) for tool configuration.

Build the compiler toolchain and install the CLI from a complete checkout:

```sh
git clone https://github.com/andrewarrow/pactern.git
cd pactern
cargo build --workspace --locked
cargo install --path crates/pactern-cli --locked
export PATH="$HOME/.cargo/bin:$PATH"
pactern version --json
```

Create a separate project with a function that returns `42` and a pact that
checks its result:

```sh
mkdir -p ../hello-pactern/src ../hello-pactern/contracts
cd ../hello-pactern

cat > src/main.ptn <<'EOF'
module main;

pub fn main() -> I64 {
    return 42;
}
EOF

cat > contracts/main.pact.ptn <<'EOF'
module main;

pact answer_is_42 for main {
    ensures result == 42;
}
EOF
```

Check the project, compile your first WebAssembly binary, and run the program:

```sh
pactern check
pactern build --entry main::main --output target/hello.wasm
pactern run --entry main::main --json
```

`check`, `build`, and `run` select `src` and `contracts` automatically. The build
writes `target/hello.wasm` and its generated C source, `target/hello.c`. The run
report includes `"outcome":{"kind":"return","value":"42"}`; I64 values use
JSON decimal strings. The pact is checked at runtime by default.

The current binary target is WebAssembly. `pactern run` recompiles the selected
source and executes it in the bundled sandboxed host; it does not load the
previously built `.wasm` file. Artifacts currently embed their entry arguments.
For a function with I64 parameters, pass arguments as strings, for example
`--args '["3","2"]'` to both `build` and `run`.

To prove the pact as well, install Z3 (the candidate suite uses 5.3.1), then run
`pactern verify`. Z3 is not needed for the check/build/run steps above. See the
[development guide](docs/development.md) for tests and full toolchain checks.

## Why write the rules twice?

Contracts repeat some business knowledge. That is a cost, and the independent
check needs to justify it. In the small [quota example](docs/spec/0.1.0/examples.md),
the implementation and pact both describe valid amounts, insufficient quota,
and the remaining balance. For a function that small, the repetition is conspicuous.

The implementation computes an answer. The pact states which answers are
acceptable. Sharing the entire result-producing helper between them can defeat
the check: if both use the same faulty calculation, they can agree on the wrong
answer. This is the same reason a useful test needs an expected result that does
not simply call the function under test again.

Separate files let requirements have different change rules from implementations.
Under the acceptance policy, an agent can repair the code while the
accepted pact and its meaning-defining helpers stay protected. A legitimate
requirement change gets its own approval, tied to the exact diff.

That does not make an exhaustive pact worthwhile for every small helper.
The strongest use is an important boundary where a few stable properties can
constrain many implementation decisions. Ordinary code reuse still belongs
inside the implementation; the contract needs enough independence to catch its
mistakes.

## Example: allocating workers across two queues

Suppose a scheduler has a fixed worker budget and two queues with different
amounts of ready work. It should split workers evenly, give a queue no more
workers than it needs, and transfer unused capacity to the other queue. It should
leave workers idle only when both queues have all the workers they need.

Here is one implementation in `src/scheduler.ptn`. It takes an initial split,
caps each allocation at demand, then redistributes the spare capacity:

```text
module scheduler;

pub struct Allocation {
    a: I64,
    b: I64,
}

pub fn allocate_workers(budget: I64, demand_a: I64, demand_b: I64) -> Allocation {
    let mut a = budget / 2;
    if (a > demand_a) {
        a = demand_a;
    }

    let mut b = budget - a;
    if (b > demand_b) {
        b = demand_b;
    }

    let spare = budget - a - b;
    if (spare > demand_a - a) {
        a = demand_a;
    } else {
        a = a + spare;
    }

    return Allocation { a: a, b: b };
}
```

The pact in `contracts/scheduler.pact.ptn` specifies the acceptable allocation:

```text
module scheduler;

pact allocation_rules for allocate_workers {
    requires budget >= 0 && demand_a >= 0 && demand_b >= 0;

    ensures result.a >= 0 && result.a <= demand_a;
    ensures result.b >= 0 && result.b <= demand_b;
    ensures result.a <= budget && result.b <= budget - result.a;

    ensures result.a + result.b == budget ||
        (result.a == demand_a && result.b == demand_b);

    ensures result.a - result.b <= 1 || result.b == demand_b;
    ensures result.b - result.a <= 1 || result.a == demand_a;
}
```

The clauses require valid allocations, respect for the total budget, no wasted
capacity while demand remains, and a balanced split. A queue can receive more
than one extra worker only when the other queue's demand is fully met. The
budget check uses subtraction so that checking a bad allocation cannot overflow
by adding two large values; the later sum is safe after that check passes.

Notice what the pact leaves open. It does not mention the initial half split,
the order of the demand checks, or the redistribution step. With five workers
and two busy queues, both `(2, 3)` and `(3, 2)` satisfy it. An agent could change
which queue receives the odd worker or rewrite the allocation algorithm while
preserving the same requirements.

Those requirements catch several different implementation mistakes:

| Inputs `(budget, demand_a, demand_b)` | Faulty result `(a, b)` | Violated requirement |
| --- | --- | --- |
| `(10, 100, 2)` | `(5, 2)` after omitting redistribution | Three workers sit idle while queue A has work. |
| `(5, 100, 100)` | `(3, 3)` after rounding both shares up | Six workers are allocated from a budget of five. |
| `(10, 100, 100)` | `(10, 0)` | The split is unbalanced while queue B still needs workers. |
| `(10, 100, 100)` | `(0, 0)` | Returning nothing fails the requirement to use available capacity. |

This is where the duplication earns its place: the pact describes properties of
the result independently of the steps that produce it. A repair has to restore
those properties without weakening them. The pact still needs review—it covers
one allocation decision, not fairness over time, job completion, or concurrent
updates to queue state.

The example uses the loop-free integer and record subset targeted by the
[language specification](docs/spec/0.1.0/language.md). The frontend parses, resolves, and type-checks this source. The interpreter and
Wasm backend execute it; the narrow verifier reports postconditions, arithmetic
traps, and termination as separate claims. Unsupported code and solver timeouts
remain unsupported or unknown.

## Why Pactern's approach fits agent development

A pact connects three things that belong together: **required behavior,
permitted authority, and evidence for acceptance**. The design follows that
agreement through the entire change.

**Requirements survive the repair loop.** Protected contracts and acceptance
criteria are checked against a trusted baseline outside the candidate change.
That check also covers ordinary text edits and the helpers that give a contract
its meaning. An agent shouldn't be able to redefine success by editing the file
that tells CI what to accept.

**Agents work with the compiler's understanding of the project.** The
semantic API exposes signatures, callers, effects, and affected contracts.
Changes name a target and a base revision, then validate in an isolated candidate
before an atomic commit. A stale proposal is rejected before it can overwrite
newer work. Readable `.ptn` files remain the source of truth, so ordinary editors
and Git review remain part of the workflow.

**Permissions hold at execution time.** Effect declarations make requested
operations visible in source. Scoped handles issued by the host authorize those
operations for specific resources or tenants. The WebAssembly host
denies ambient access and enforces execution, memory, and operation budgets.
Declaring `db.write` cannot grant permission to write another customer's data.
For the quota service, transactional storage must separately enforce atomic
updates and tenant isolation.

**Evidence says what was actually checked.** Static proofs, runtime guards, and
executed tests have separate results. A timeout stays unknown; an unsupported
proof stays unsupported. Evidence is designed to identify the exact source,
artifact, toolchain, and policy it covers. A reviewer should be able to see both
what passed and what remains unestablished.

## Why build a language for this?

The compiler and verifier need to agree on what a program means. So do the
semantic editor and runtime. Pactern puts those responsibilities around one
small language and a shared representation. The current resolver builds
file-aware symbol and reference indexes, and the core type checker infers value
types with source origins. The typed core representation is shared by execution, C lowering, and verification.

For example, `I64` arithmetic is specified to trap on overflow in every build
mode. The interpreter, generated code, and prover must all preserve that rule.
A proof over unlimited mathematical integers would say too little about an
implementation that executes fixed-width arithmetic.

The language uses familiar functions, braces, modules, `Option` and `Result`,
with immutable values by default and explicit local mutation. The
implementation uses Rust for the toolchain, a reference interpreter to check
semantics, and C lowering to WebAssembly for deployment.

This is the case for Pactern: the same agreement should guide the edit, the
checks, and the resulting program. The project will measure whether that makes
correct changes cheaper and easier to review against strong Rust and Go
workflows. Comparative results are still ahead of us.

## Start here

The first target is sandboxed backend components: business rules, validation,
data transformations, and bounded jobs called by an existing application. The
quota demo is the first test of the complete design.

Follow the [quickstart](#quickstart) to build the toolchain and run a first program.
`pactern check` and `pactern fmt` select `src` and `contracts` by default.
`verify` emits separate SMT claims; `build` produces a guarded Wasm artifact;
`run` compiles and executes it under memory/fuel limits; `test` compares seeded
properties in the interpreter and Wasm host. I64 arguments use exact JSON decimal
strings. For example, after building:

```sh
(cd fixtures/conformance && ../../target/debug/pactern run \
  --entry quota::reserve --args '["3","2"]' \
  src/quota.ptn contracts/quota.pact.ptn --json)
cargo run -p pactern --example quota_workflow --locked -- target/quota-workflow
```

The quota workflow exposes and repairs a defect, rejects protected and stale
changes, executes the compiled rule inside a tenant transaction, replays a failure
without writes, and emits artifact-linked evidence. `inspect`, `callers`, `impact`,
and the `editor` adapter use the shared semantic index. `init` and `apply` operate
on a managed workspace with an externally supplied baseline; ordinary source-tree
files are not silently overwritten by semantic transactions.

This is an experimental source candidate. Wasm arguments are currently specialized
into each artifact; the storage adapter is in-memory; a complete LSP extension is
not included. Runtime smoke benchmarks have been rerun, but matched agent repair
ablations remain unmeasured. The recorded decision is to narrow the preview.
See [milestones](MILESTONES.md) for exact completion boundaries.

- [Language specification](docs/spec/0.1.0/README.md): semantics, grammar, and protocol contracts.
- [Design plan](PLAN.md): architecture, tradeoffs, and evaluation criteria.
- [Development guide](docs/development.md): workspace layout and checks.
- [CLI reference](docs/cli/0.1.0/README.md): commands, JSON output, and exit codes.

Pactern is pronounced **PACK-turn**. Licensed under [Apache 2.0](LICENSE).
