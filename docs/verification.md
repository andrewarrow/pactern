# Pacts, verification and artifact evidence

The `pacts`, `verification`, and `evidence` library modules implement the narrow
v0.1 verification path. Run `cargo test -p pactern --test pacts --locked` with
Z3 and the C/Wasm toolchain installed. Tests exercise all 17 recorded runtime
pact cases in the interpreter and generated Wasm, deterministic seeded quota
properties, the genuine unbounded quota proof, both defective implementations,
checked-integer traps, inconsistent premises, unsupported loops, solver timeout,
and evidence invalidation. Solver-dependent tests explicitly skip when Z3 is
unavailable; release validation must install Z3 before running this suite.

## Runtime checks and inventory

`interpreter::execute_guarded` runs every attached precondition before a function
body and every postcondition after a normal return, including internal calls.
Checks follow source/declaration/clause order and preserve short-circuiting.
False clauses produce `Precondition` or `Postcondition`; evaluation traps produce
`ContractEvaluation` with the underlying trap. Body traps bypass postconditions.
`execute` remains the explicitly contract-free reference API for compatibility
and benchmark ablations. Product entry points must use the guarded API.

`pacts::inventory(program, revision)` binds clause IDs to the supplied exact
input revision, pact identity within that revision, path and byte location. It
separates statically supported and unsupported targets while retaining runtime
guard eligibility. `inventory_verified` additionally labels solver-detected
inconsistent preconditions. The caller supplies a complete trusted revision;
a source-only diagnostic snapshot is not a dependency/policy inventory.

`pacts::compile_guarded` emits C with the backend's guarded mode, compiles it to
Wasm, and returns a guard map bound to the module's SHA-256 digest. The map covers
clauses on transitively reachable functions, including contract helpers. It does
not advertise checks on unrelated functions removed by the linker. The initial
ABI binds the entry and arguments into each invocation artifact; it is not a
reusable arbitrary-input component. No proof-based guard elimination is enabled.

`run_examples` compares explicit expected values/traps and compiled outcomes.
`run_properties` generates bounded deterministic `I64`/`Bool` cases using a
recorded seed, edge values and SplitMix64. False preconditions are counted as
discards; postcondition/evaluation failures fail the property. A capped discard
budget prevents inconsistent premises from hanging the runner; incomplete runs
fail. Every report records the checked-program fingerprint, requested/executed counts, seed, environment, engine,
inputs and results. The compiled callback must run the real artifact. These are
executed tests, not universal proofs or a Pactern source property syntax.

## Static fragment and solver

`verification::queries` emits reviewable SMT-LIB and `verify` invokes an external
Z3 executable. Set `Solver.executable` and `timeout_ms` explicitly when needed.
Solver output, process duration, call expansion, symbolic paths and term sizes
are bounded. Missing solvers, timeouts and unrecognized responses remain unknown.
Satisfiable counterexample queries retain a solver model when available; `arg_N`
refers to the parameter's resolution-local symbol ID.

The supported fragment is loop-free pure nongeneric functions with acyclic
inlined calls, `Bool`, checked `I64`, Unit, finite records and enums, including
`Option` and `Result`. Generic target/helper functions, strings, lists, indexing,
recursive data, effects, loops and recursion return unsupported. Concrete
instantiations of finite generic data are supported. Expansion limits can also
produce unsupported; they never produce a bounded proof.

Integers are 64-bit bit-vectors. Addition, subtraction and multiplication compare
128-bit sign-extended arithmetic with the sign-extended result to detect exact
signed overflow. Negation excludes the minimum integer. Division and remainder
exclude zero and `MIN / -1` or `MIN % -1`. Comparisons are signed. Safety predicates
follow evaluated paths, short-circuit operators, matches, eager unused bindings,
internal helper guards and contract-clause ordering. SMT datatypes preserve
constructor distinctions and record/payload structure.

Reports separate precondition consistency, postcondition validity, absence of
language traps and termination. Inconsistent preconditions reject vacuous
proofs. The postcondition claim concerns normal, safe returns; it cannot replace
the separate trap claim. The trap query also checks precondition-evaluation
safety. Structural termination assumes sufficient fuel, stack and memory.
External solver results and compiler correctness remain explicit trusted
assumptions; no independently checked proof certificate is produced.

## Evidence and trusted acceptance

The JSON shape is documented in [evidence.schema.json](evidence.schema.json).
`Binding::new` binds the checked-program fingerprint and hashes exact source, dependency, compiler executable, solver
executable, artifact and policy bytes, and records versions, target and
assumptions. `Evidence::reusable_for` compares every binding field. Callers must
supply the complete trusted input inventory, including runtime, lockfiles,
protected helpers, fixtures, toolchain and relevant configuration. A version
string is not a substitute for executable bytes.

`Evidence::evaluate` keeps unbounded proofs, compiler-produced runtime guard
maps and executed tests separate. Any refuted claim rejects the candidate.
Proof and test reports must match the complete checked-program fingerprint, preventing reuse across unrelated programs with coincidentally equal local symbol IDs. Required proof needs all four claims proved with no exploration bound. Missing,
unknown and unsupported proof cannot be replaced by guards or tests. Required
tests must execute a nonempty compiled/interpreter suite successfully. Missing,
duplicate or wrong-artifact guard evidence is rejected.

These JSON records are not signed certificates. The trusted external acceptance
tool must construct the report from the actual compiler/solver/test executions
and independently compute the expected binding and policy. Candidate-supplied
JSON, inventories, callbacks and approvals do not confer acceptance authority.

## Scripted quota workflow

Run the complete local demonstration:

```sh
cargo run -p pactern --example quota_workflow --locked -- target/quota-workflow
```

It inspects the persistent semantic identity and affected context, obtains an SMT
counterexample for unchecked quota, rejects protected-pact edits through both
text and semantic paths, stages a body repair with required real proofs, and
rejects a concurrently staged stale repair. It then executes guarded Wasm inside
an explicit Rust-hosted tenant transaction, checks tenant isolation, and replays
the original guard failure against its old revision with zero live writes.

The output directory contains the generated C/Wasm, failure artifact, host traces,
refutation, repair request/receipt, workspace snapshot, policy, and `evidence.json`.
The evidence hashes the actual running compiler/host executable, actual Z3,
clang and wasm-ld binaries, runtime, lockfile, toolchain configuration, identity
ledger, protected baseline, grants, budgets, and every compiled test artifact.
The required examples and seeded properties run against the interpreter and
actual compiled artifacts before acceptance.

This is a deterministic scripted workflow, not a measured autonomous-agent
repair experiment. The Rust host invokes the pure rule explicitly through the import-free ABI; this
workflow does not use a guest host-import bridge. The demonstration
uses the in-memory optimistic transactional adapter, not a persistent SQL service.
