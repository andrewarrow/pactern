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

**Early development:** the specification, Rust library, and installable CLI
skeleton are available. Compilation, verification, execution, and semantic edits
remain to be implemented. The examples and workflow below describe the design.

## A bug fix should preserve the requirement

Suppose your service reserves capacity from a customer's quota. Valid requests
must succeed, invalid requests must return the right error, and the remaining
quota must never become negative.

The implementation lives in `src/quota.ptn`:

```text
module quota;

pub enum ReserveError {
    InvalidAmount,
    InsufficientQuota,
}

pub fn reserve(available: I64, amount: I64) -> Result<I64, ReserveError> {
    if (amount <= 0) {
        return Result::Err(ReserveError::InvalidAmount);
    }
    if (amount > available) {
        return Result::Err(ReserveError::InsufficientQuota);
    }
    return Result::Ok(available - amount);
}
```

Its agreement lives separately in `contracts/quota.pact.ptn`:

```text
module quota;

pact reserve_safety for reserve {
    requires available >= 0;
    ensures match (result) {
        Result::Ok(remaining) =>
            amount > 0 && amount <= available &&
            remaining == available - amount && remaining >= 0,
        Result::Err(ReserveError::InvalidAmount) => amount <= 0,
        Result::Err(ReserveError::InsufficientQuota) =>
            amount > 0 && amount > available,
    };
}
```

If an agent removes the insufficient-quota check, reserving four units from
three would return `Ok(-1)`. That violates the pact. Returning an error for every
request would violate it too: reserving two units from three must return `Ok(1)`.

The planned verification loop gives the agent a concrete counterexample it can
use to repair the implementation. The acceptance gate protects the pact, its
helpers, and the required tests. Changing those requirements takes a separate,
explicitly approved specification change, tied to the exact proposed diff.

These examples follow the [0.1.0 specification](docs/spec/0.1.0/examples.md).
They are not executable with the current CLI.

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
[implementation milestone](MILESTONES.md): conformance fixtures and reproducible
baseline tasks. Concrete edge cases, small failing programs, and challenges to
the specification are useful contributions alongside compiler work.

- [Language specification](docs/spec/0.1.0/README.md): semantics, grammar, and protocol contracts.
- [Design plan](PLAN.md): architecture, tradeoffs, and evaluation criteria.
- [Development guide](docs/development.md): workspace layout and checks.
- [CLI reference](docs/cli/0.1.0/README.md): commands, JSON output, and exit codes.

Pactern is pronounced **PACK-turn**. Licensed under [Apache 2.0](LICENSE).
