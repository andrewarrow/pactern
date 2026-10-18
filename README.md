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

**Early development:** the specification, Rust library, installable CLI skeleton,
conformance fixtures, and Go/Rust baseline harness are available. Pactern
compilation, verification, execution, and semantic edits remain to be implemented.
The examples and workflow below describe the design.

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
Under the planned acceptance policy, an agent can repair the code while the
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
[language specification](docs/spec/0.1.0/language.md). It is illustrative source;
the compiler and verification pipeline are not implemented yet. The planned
pipeline would check postconditions, arithmetic traps, and termination as
separate claims, and report counterexamples when it refutes a supported claim.

## Why Pactern's approach fits agent development

A pact connects three things that belong together: **required behavior,
permitted authority, and evidence for acceptance**. The design follows that
agreement through the entire change.

**Requirements survive the repair loop.** Protected contracts and acceptance
criteria are checked against a trusted baseline outside the candidate change.
That check also covers ordinary text edits and the helpers that give a contract
its meaning. An agent shouldn't be able to redefine success by editing the file
that tells CI what to accept.

**Agents work with the compiler's understanding of the project.** The planned
semantic API exposes signatures, callers, effects, and affected contracts.
Changes name a target and a base revision, then validate in an isolated candidate
before an atomic commit. A stale proposal is rejected before it can overwrite
newer work. Readable `.ptn` files remain the source of truth, so ordinary editors
and Git review remain part of the workflow.

**Permissions hold at execution time.** Effect declarations make requested
operations visible in source. Scoped handles issued by the host authorize those
operations for specific resources or tenants. The planned WebAssembly host
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
small language and a shared typed representation.

For example, `I64` arithmetic is specified to trap on overflow in every build
mode. The interpreter, generated code, and prover must all preserve that rule.
A proof over unlimited mathematical integers would say too little about an
implementation that executes fixed-width arithmetic.

The language uses familiar functions, braces, modules, `Option` and `Result`,
with immutable values by default and explicit local mutation. The planned
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

You can build and inspect the current toolchain now. The repository pins Rust
1.98.1 through `rust-toolchain.toml`.

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo run -p pactern-cli -- version --json
cargo run -p pactern-cli -- --help
```

To install the CLI from this checkout:

```sh
cargo install --path crates/pactern-cli --locked
pactern version --json
```

Help, version metadata, and structured tool diagnostics work today. Commands
such as `check`, `verify`, `test`, `build`, and `apply` currently return explicit
not-implemented errors. They do not process or modify a Pactern project.

If you want to help build the language, start with the next
[implementation milestone](MILESTONES.md): source spans, lexer, and parser.
The [conformance corpus](fixtures/conformance/README.md) and
[Go/Rust seed baselines](baselines/README.md) provide recorded expectations and
reproducible measurements. Concrete edge cases, small failing programs, and challenges to
the specification are useful contributions alongside compiler work.

- [Language specification](docs/spec/0.1.0/README.md): semantics, grammar, and protocol contracts.
- [Design plan](PLAN.md): architecture, tradeoffs, and evaluation criteria.
- [Development guide](docs/development.md): workspace layout and checks.
- [CLI reference](docs/cli/0.1.0/README.md): commands, JSON output, and exit codes.

Pactern is pronounced **PACK-turn**. Licensed under [Apache 2.0](LICENSE).
