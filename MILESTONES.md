# Planned implementation milestones

These are sequential planned milestones; each has an observable exit criterion. Work may proceed in parallel only where prerequisites are ready. See [PLAN.md](PLAN.md) for the design rationale and v0.1 scope.

Rust is the compiler, tooling, and sandbox-host language. The Rust compiler lowers Pactern to generated C, which is compiled to WebAssembly; a small C guest runtime supplies the runtime ABI and garbage collection. SMT support means Rust emits SMT-LIB for an external solver. Markdown, EBNF, JSON, TOML, and YAML below are data or documentation formats, not language implementations. Completion status is recorded below; unmarked milestones remain planned.

These 30 v0.1 milestones exclude compiler self-hosting, general concurrency, broad FFI, and a production native backend. Earlier completion notes record the state at the time that milestone landed; later sections supersede their “future work” boundaries. The current candidate is experimental, and an implemented local gate is distinct from activated external CI protection.

## Specify and measure

### 1. Specify the first language and protocol contracts

**Status:** Complete as a reviewable specification draft. See the [0.1.0 specification bundle](docs/spec/0.1.0/README.md) for the language semantics, grammar, protocol schemas, and edge-case examples. No compiler or runtime implementation is claimed.

**Languages/formats:** Markdown specification, EBNF grammar, JSON Schema documents. No code implementation. **Done when:** the versioned core semantics, diagnostic and change-request schemas, effect boundary, and supported subset are reviewable, with examples resolving edge cases for integers, errors, handles, and contracts.

### 2. Establish the Rust workspace and CLI skeleton

**Status:** Complete. The Rust workspace provides the shared `pactern` library
and an installable `pactern` CLI with versioned help/version JSON, protocol 0.1.0
tool diagnostics, and explicit unavailable-command errors. See the
[development guide](docs/development.md) and [CLI contract](docs/cli/0.1.0/README.md).
A clean source copy builds with the lockfile, and a temporary installation runs
outside the checkout. Library, CLI integration, and documentation tests pass;
formatting, Clippy, and emitted JSON schema validation pass. No compiler, runtime,
verification, or semantic-edit implementation is claimed.

**Languages/formats:** Rust; TOML manifests. **Done when:** a clean checkout builds an installable `pactern` CLI and library with versioned command and JSON-output entry points.

### 3. Seed conformance fixtures and baseline tasks

**Status:** Complete. The [conformance corpus](fixtures/conformance/README.md)
records 79 cases covering values, diagnostics, traps, pact guards, and separate
verification expectations. The Rust baseline harness validates the corpus and
checks 25 selected execution cases against independent Go/Rust programs for quota
reservation and two transformations. The [baseline guide](baselines/README.md)
documents reproduction and the checked-in initial measurements of build time,
process/batch time, and artifact size. Agent/token costs remain unmeasured; these
public seed tasks are not a held-out agent evaluation. No Pactern execution or
verification is claimed.

**Languages/formats:** Pactern fixtures; Rust baseline harness; Go and Rust comparison programs; JSON task data. **Done when:** fixture inputs have recorded expected values, diagnostics, traps, and pact outcomes, and the Rust harness reproduces Go/Rust baseline results and costs before Pactern execution exists.

## Build the language path

### 4. Implement source spans, lexer, and parser

**Status:** Complete for syntax. The `pactern` library exposes a source file, lexer, and parser for implementation and contract files. Valid corpus inputs produce syntax trees with UTF-8 byte offsets and line/scalar-column spans; malformed fixtures and deterministic mutation, random-input, and truncation smoke cases return first-error located diagnostics without panicking. The lexer caps source files at 1 MiB and emitted tokens at 100,000 including EOF; parser nesting and expression depth are capped at 64. These are syntax-only local diagnostics, not protocol envelopes. Typing, execution, and verification remain future work.

**Languages/formats:** Rust; Pactern source and EBNF grammar. **Done when:** valid fixture programs produce syntax trees with byte and line/column spans, while malformed and fuzz-generated input returns bounded, located diagnostics without crashing.

### 5. Resolve modules and names

**Status:** Complete. `pactern::resolution::resolve` accepts parsed files identified by package-relative paths and returns file, module, and symbol indexes, file-aware references, deferred uses, and diagnostics with related declaration locations. It resolves file-scoped qualified imports, visibility, duplicates and shadowing, unresolved names, dependency cycles across implementation and contract files, constructor and generic-parameter names, and pact target/parameter/result scopes. The 24 resolver tests and staged conformance cases pass with `cargo test --workspace --locked`; workspace formatting and Clippy checks pass. IDs belong to one resolution result, and resolution performs no filesystem or network access. Field accesses and effect labels are deferred; type/effect checking, generic recursion, handles, and host limits remain later work.

**Languages/formats:** Rust; Pactern modules. **Done when:** imports, visibility, shadowing, duplicate names, and unresolved names match fixtures and diagnostics point to the relevant declarations and uses.

### 6. Type-check the core language

**Status:** Complete for core value typing. `pactern::typing::check` resolves a
closed set of parsed files, then checks nominal records and enums, prelude
containers, integer literal ranges, operators, calls, local inference, immutable
parameters/bindings and explicit mutation, returns, loop control, and Boolean pact
clauses. Nested pattern coverage checks both exhaustiveness and unreachable arms.
Results expose file-aware diagnostics, symbol types, and expression types using
resolution-local IDs. The 19 typing tests exercise staged conformance expectations,
1,000 match combinations against an independent finite-domain oracle, and analysis
limits; workspace tests, build, formatting, and Clippy pass. User-defined generics
and borrowed handles receive explicit unsupported diagnostics. Effects, typed core
IR, execution, verification, protocol envelopes, and CLI project loading remain
later work. See the [development guide](docs/development.md) for the API and limits.

**Languages/formats:** Rust; Pactern source. **Done when:** fixtures establish typing for `Bool`, checked `I64`, UTF-8 strings, lists, records, tagged unions, `Option`/`Result`, and exhaustive matching, plus local inference and explicit public signatures. The checker accepts immutable-by-default bindings and explicit local mutation, and rejects examples that violate these rules.

### 7. Add simple generic functions and types

**Status:** Complete for value typing. The checker supports rank-1 functions,
records, and enums with rigid generic body parameters, independent call-site
inference, explicit type arguments, and substituted field/pattern types. Generic
call cycles are rejected, including cycles through nongeneric helpers; ambiguous
inference and invalid arities receive located diagnostics. Public type results
retain nominal arguments and generic parameter IDs. The typing suite covers
imported generics, nested coverage, recursive data, invalid body operations, and
resource limits. Workspace tests, build, formatting, and Clippy pass. See the
[development guide](docs/development.md#generic-value-typing) for examples and
boundaries. Monomorphization, typed core IR, handles, and execution remain later work.

**Languages/formats:** Rust; Pactern source. **Done when:** the documented generic cases type-check and unsupported inference or specialization cases receive explicit diagnostics rather than silently changing types.

### 8. Define a typed core IR

**Status:** Complete for the supported value language. `pactern::core::lower`
checks a closed package and produces immutable, structured generic IR with
file-aware source origins, typed expressions and patterns, resolved symbol/field
IDs, function signatures, data declarations, and typed pact clauses. Lowering
requires complete annotations and references after successful resolution and
typing; failures return diagnostics or located compiler-invariant errors, never
partial IR. Calls retain ordered inferred type arguments, records retain source
field evaluation order, and short-circuit/control-flow operations stay explicit.
Seven core integration tests and the accepted typing/conformance cases cover the
lowering path. Workspace tests, build, formatting, and Clippy pass. See the
[development guide](docs/development.md#typed-core-ir) for invariants and consumer
semantics. Interpretation, monomorphization, effect/handle safety, protocol
serialization, and CLI project loading remain later work.

**Languages/formats:** Rust; serialized debug IR as JSON where useful. **Done when:** parsed and resolved programs lower into a typed representation that records source origins and has validated invariants for every supported construct.

### 9. Build the pure reference interpreter

**Status:** Complete for pure value execution. `pactern::interpreter::execute`
runs checked core IR with validated entry arguments, generic helpers, immutable
values, local mutation, structured control flow, matching, and located arithmetic
and bounds traps. All 48 pure-runtime conformance cases pass, covering the quota
helper, both transformations, checked arithmetic, and evaluation order. Additional
tests cover generic data, nested patterns, recursion, invalid calls, and fuel/value
depth limits. Workspace tests, build, formatting, and Clippy pass. See the
[development guide](docs/development.md#pure-reference-interpreter) for API and
accounting limits. Pact guards, effects/handles, compiled execution, verification,
and CLI project loading remain later work.

**Languages/formats:** Rust; Pactern programs. **Done when:** the quota helper and transformation fixtures execute from typed IR with specified values, errors, checked-overflow traps, and evaluation order.

### 10. Add formatting and structured diagnostics

**Status:** Complete for the source-only frontend. `pactern::formatting::format`
preserves exact tokens, comments, and literal spellings while producing stable
four-space/LF layout; parseable corpus inputs reparse with identical tokens and
are idempotent. `pactern check [PATH ...]` and `pactern fmt [--check] [PATH ...]`
load bounded source selections and emit protocol 0.1.0 diagnostics with stable
codes, revision-bound UTF-8 byte spans, and related declarations. Snapshot hashes
bind the selected source bytes and frontend profile, not future trusted acceptance
inventories. Persistent symbol IDs remain milestone 21 work and are omitted rather
than fabricated from local IDs. Nine formatter tests, six snapshot tests, thirteen
CLI tests, workspace build/tests, formatting, and Clippy pass; 168 emitted CLI
documents validate against the versioned schemas. See the
[CLI contract](docs/cli/0.1.0/README.md) and
[development guide](docs/development.md#formatting-and-source-diagnostics).

**Languages/formats:** Rust; Pactern source; JSON diagnostic schema. **Done when:** formatting is stable under repeated runs and CLI diagnostics conform to the versioned schema with stable codes, spans, and relevant symbol IDs.

### 11. Track effects and noncopyable handles

**Status:** Complete for the registered quota handle subset. The frontend checks
all declared effect bounds across local/imported calls, rejects unknown/duplicate
effects and effectful pact helpers, and checks moves, conservative branch joins,
loop consumption, exclusive argument borrows/reborrows, aggregate/generic escapes,
and immutable handle bindings. The quota registry has an opaque Txn type and
read/reserve/commit/rollback signatures. Adversarial cases and diagnostics are
covered by `tests/effects.rs`; unavailable adapters remain errors. Every Pactern
implementation and contract now has a hard 1,000-physical-line limit, including
comments and blanks, also enforced on formatter output. See [host rules](docs/host.md).

**Languages/formats:** Rust; Pactern source. **Done when:** static checks propagate effects through calls and imports, reject effects that exceed a declaration's public bounds, and reject copying, escaping, or using scoped handles after their permitted lifetime in supported cases.

## Lower to a sandboxed executable

### 12. Implement the C guest runtime and ABI

**Status:** Complete for the pure guest ABI. The freestanding C runtime provides
tagged immutable values, checked arithmetic and bounds, precise shadow roots,
nonmoving mark/sweep collection, explicit trap/cleanup paths, and bounded output.
Native ASan/UBSan tests and forced-collection Wasm cases exercise reclamation and
live-root retention. See [the runtime ABI](docs/backend.md).

**Languages/formats:** C guest runtime; Rust ABI definitions and test driver. **Done when:** runtime tests demonstrate allocation, precise root tracking and collection, value representation, checked traps, and cleanup behavior under the documented ABI.

### 13. Emit C from typed core

**Status:** Complete for the supported pure core. Generated C covers generic
helpers/data, records in source evaluation order, enums, lists, strings, nested
patterns, mutation, loops, short-circuiting, and checked arithmetic. All 48 recorded
pure runtime cases agree with the reference interpreter through actual C/Wasm
compilation. Logical-fuel and multiplication edge regressions are retained in
`tests/backend.rs`. See [backend limits](docs/backend.md).

**Languages/formats:** Rust C emitter; generated C output. **Done when:** supported pure fixtures compile as generated C and agree with the interpreter for values, evaluation order, arithmetic traps, bounds checks, and GC roots.

### 14. Compile C to Wasm and add a minimal Rust host

**Status:** Complete for a core-Wasm prototype. Clang/wasm-ld produces actual
modules and the Rust Wasmi host validates them, denies all ambient imports by
default, and enforces module/output size, memory, and fuel limits. Malformed
modules, import rejection, and exhaustion are tested. Invocation arguments are
currently specialized into an artifact; a reusable argument ABI and WebAssembly
Component Model packaging remain outside this prototype.

**Languages/formats:** Rust build and sandbox host; generated C; WebAssembly target. **Done when:** a built component runs under a host that denies all ambient imports by default and enforces memory and execution limits, with rejected imports and limit traps covered by runtime checks.

### 15. Establish continuous differential and fuzz validation

**Status:** Complete for initial continuous validation. The full-suite script and
CI definition execute corpus differential checks, deterministic arithmetic cases,
parser/type/lowering mutation smoke tests, hostile module checks, and sanitized
native runtime tests. Found arithmetic/fuel/guard regressions are kept as small
executable tests. This is bounded deterministic coverage, not a claim that a
long-running fuzz campaign has exhausted the language.

**Languages/formats:** Rust harnesses and compiler; Pactern generated inputs; C/Wasm artifacts. **Done when:** routine validation compares interpreter and Wasm outcomes, fuzzes parser/type/lowering/runtime boundaries, and preserves minimized regressions for every discovered mismatch.

## Check pacts and produce evidence

### 16. Parse pacts and inventory obligations

**Status:** Complete. Resolved pacts produce source/dependency-revision-bound
obligation inventories with per-clause locations and explicit support reasons.
Solver-enriched inventories mark inconsistent premises without claiming vacuous
proof. See [verification](docs/verification.md).

**Languages/formats:** Rust; Pactern contract files; JSON reports. **Done when:** contracts attach to resolved declarations and produce revision-scoped obligation IDs with explicit supported, unsupported, and inconsistent cases.

### 17. Add example and property execution

**Status:** Complete for explicit examples and seeded I64/Bool properties.
Reports retain requested/executed/discarded counts, arguments, seeds, environment,
failures, and separate interpreter/compiled outcomes. Compiled callbacks execute
actual guarded Wasm; missing runs cannot satisfy required tests. Property testing
is kept distinct from proof. The CLI exposes reproducible seed/case controls.

**Languages/formats:** Rust runner; Pactern examples and properties; JSON seeds/results. **Done when:** deterministic examples and seeded properties execute against interpreter and compiled forms, recording failures, seeds, and environment.

### 18. Emit runtime contract guards

**Status:** Complete for the compiled pure fragment. Interpreter and generated C
check preconditions, postconditions on ordinary and recoverable error returns,
internal calls, short-circuiting, and contract-evaluation traps. The guarded build
maps reachable obligations to the exact artifact. All 17 recorded guard cases run
in Wasm and match expected results; unguarded execution remains an explicit ablation.

**Languages/formats:** Rust compiler; generated C and Wasm; Pactern contracts. **Done when:** each advertised runtime-enforced obligation maps to a check in the artifact, and deliberately violated inputs fail with the expected guard result.

### 19. Integrate narrow SMT verification

**Status:** Complete for the narrow verifier. An external Z3 process checks
64-bit vectors with explicit overflow, division/remainder, and contract-evaluation
safety conditions, plus finite records/enums/Option/Result and acyclic helper calls.
The quota implementation receives real unbounded proofs; both faulty variants
are refuted. Reports separate consistency, postconditions, trap freedom, and
termination. Missing/nonresponsive solvers stay unknown; loops, recursion,
generic targets, and unsupported data stay unsupported. No bounded search is
represented as an unbounded proof. See [the exact fragment](docs/verification.md).

**Languages/formats:** Rust obligation generator; SMT-LIB bit-vector and datatype encodings; external solver. **Done when:** exact checked machine-`I64` semantics, including overflow and other specified traps, are encoded for loop-free pure code; supported enums and `Option`/`Result` suffice to verify the quota example. Reports separately state postcondition validity, absence of traps, and termination; bounded exploration is labelled with its bound and remains distinct from unbounded proof, while timeout and unsupported cases remain unknown or unsupported.

### 20. Bind evidence to artifacts and policy

**Status:** Complete for the library evidence/acceptance path. Source and locked
dependency content, compiler/solver bytes and versions, target, artifact, policy,
and assumptions participate in exact evidence binding. Proofs, generated guard
maps, and actual executed tests remain separate. Refutations and failed required
tests reject; unknown/unsupported/bounded results cannot satisfy required proof.
Binding mutation tests and the quota workflow exercise invalidation. Evidence is
not a signature: its producer and acceptance tools must be externally trusted.

**Languages/formats:** Rust; JSON evidence schema. **Done when:** reports bind results to source/dependency hashes, compiler and solver versions, assumptions, policy, target, and artifact hash, and relevant changes invalidate cached evidence. Proof, runtime guard, and executed-test results remain separate; refuted obligations or failed required tests reject a candidate, and unknown or unsupported results cannot satisfy required proof.

## Make semantic changes reviewable and protected

### 21. Assign persistent IDs and build a semantic index

**Status:** Complete for supported declaration edits. Persisted identity ledgers
survive body edits, semantic renames, and uniquely reconcilable moves; ambiguous
matches fail for review and retired IDs are never reused. The semantic index is
rebuildable from source and ledger. Source-only queries without a ledger expose
revision-scoped provisional identities. See [semantic identities](docs/semantic.md).

**Languages/formats:** Rust; JSON identity data and rebuildable semantic index. **Done when:** declarations and pacts retain persistent IDs across supported rename and move edits, the index can be rebuilt from source and identity data, and ambiguity is surfaced for review rather than guessed silently.

### 22. Add inspection, impact, and affected-test queries

**Status:** Complete for the shared semantic API and CLI adapters. Inspection,
callers, transitive impact, and affected-test queries carry revision and
completeness information. Deferred/incomplete dependencies conservatively broaden
selection. Editor diagnostics translate byte spans to UTF-16 positions and
navigation rejects stale revisions. The CLI exposes inspect/callers/impact/editor;
property-only CLI tests explicitly report their selected-entry fallback.

**Languages/formats:** Rust library and CLI; JSON request/response schemas. **Done when:** queries return focused, completeness-marked context, affected interfaces and obligations, and conservatively broaden test selection when dependency data is incomplete.

### 23. Apply revision-checked edits atomically

**Status:** Complete for managed snapshots. Strictly decoded requests check the
full base revision, policy baseline, persistent targets, declaration hashes,
protected closure, and required validation. Memory commits publish one locked
value; durable commits atomically replace one complete snapshot under an advisory
lock. Concurrent/stale/invalid changes, retries, reopen, and multi-file rename
have tests. Ordinary editor-owned source trees are deliberately not represented
as atomically writable; the CLI imports and edits a managed store.

**Languages/formats:** Rust; JSON semantic-change requests. **Done when:** candidate edits validate against a base revision and commit only if the revision still matches; stale or invalid edits leave all workspace files unchanged.

### 24. Protect specs and enforce an external CI baseline

**Status:** Acceptance implementation and CI definition complete; external
activation remains open. Protected pacts, helpers, fixtures, and interfaces are
checked against an external exact baseline, including direct-text changes.
Specification approval binds baseline, revisions, and exact diff. CI builds the
acceptance executable from the PR base, not the candidate, and runs the full
suite separately. A baseline without the gate fails closed. The first trusted
bootstrap and required remote branch-protection settings cannot be established
by the candidate approving itself. See [activation boundaries](docs/candidate.md).

**Languages/formats:** Trusted Rust acceptance tools outside candidate changes; protected specifications, helpers, fixtures, and oracles; CI configuration (YAML). **Done when:** policy checks reject direct-text-edit bypasses and semantic changes that weaken protected requirements; approved spec changes are bound to an exact baseline and diff, with the trusted acceptance tools fixed outside candidate changes. Release CI runs the full required suite.

## Bound effects and demonstrate the workflow

### 25. Add scoped capabilities, budgets, and host cleanup

**Status:** Complete for the quota capability host and opt-in compiled import
bridge. Host-issued noncopyable tokens enforce tenant scope and read/write
grants, nested operation/byte/allocation/deadline budgets, scope expiry, cancellation,
and unwind cleanup. Guest instruction/memory limits are separate. Cleanup rolls
back uncommitted state; completed explicit commits remain committed. Generated
code transfers ownership and releases remaining handles at source-scope exits.
Compiled tests cover borrowed/owning helpers, forged and stale tokens, budgets,
timeouts, cancellation, rollback, and replay. See [host semantics](docs/host.md)
and the [compiled import ABI](docs/backend.md).

**Languages/formats:** Rust host; small C guest runtime; Pactern effect declarations; JSON policy. **Done when:** host-issued handles enforce scope and operation/byte/time/memory budgets, charge nested calls to the appropriate budget, and traps, cancellation, or timeout revoke authority and clean up host-owned resources.

### 26. Add a transactional storage adapter

**Status:** Complete for the in-memory adapter. Tenant balances are constrained
nonnegative. Transactions read a consistent version, stage reservations, and
conditionally commit under one mutex; conflicting writers retry. Sixteen-worker
concurrency tests reserve exactly the available 100 units and preserve another
tenant's balance. This is the permitted equivalent-storage-constraints prototype;
a durable SQL adapter is not claimed.

**Languages/formats:** Rust adapter and tests; Pactern wrapper; SQL schema or equivalent storage constraints. **Done when:** tenant-scoped read/update uses one transaction or conditional update, storage rejects negative quota, and concurrency tests show no over-reservation or cross-tenant access.

### 27. Record and replay host interactions

**Status:** Complete for the quota adapter. Traces bind revision and policy and
record exact ordered requests/results, including failures. Replay substitutes
recorded responses, validates shapes and complete consumption, rejects mismatched,
missing, or redacted records, and reconstructs handle liveness. Tests demonstrate
zero live writes for successful-write replay and failed-commit replay. Scope,
cancellation, and replay failures revoke authority.

**Languages/formats:** Rust host; JSON or binary trace format. **Done when:** replay substitutes recorded results, rejects request or revision mismatches, fails when required recorded data is missing or redacted, and tests verify replay performs zero live external writes.

### 28. Complete the quota demonstration

**Status:** Complete as an executable developer workflow. `quota_workflow` inspects
context, refutes the defective implementation, blocks protected weakening, applies
a proof-gated repair, rejects stale edits, runs the guarded compiled rule inside
a tenant transaction, replays an old-revision failure with zero writes, and emits
artifact-linked evidence with real executable hashes. Outputs include Wasm,
traces, evidence, and a workflow report. This scripted sequence is not a measured
agent repair experiment. See [reproduction](docs/verification.md).

**Languages/formats:** Pactern implementation, protected pact, and examples; Rust tooling/host; C runtime and Wasm output. **Done when:** a scripted agent workflow inspects relevant context, exposes and repairs a quota defect, cannot weaken protected rules, rejects stale edits, runs tenant-isolated transactional code, replays a failure without writes, and emits artifact-linked evidence.

## Prepare and decide on an alpha

### 29. Package an editor-ready reproducible candidate

**Status:** Candidate tooling implemented; final clean validation recorded in the
candidate guide. The locked source distribution builds an installable version-
matched CLI, with editor diagnostics/navigation through the shared API and CLI.
The release script copies clean source, uses fresh build/install directories,
runs the full suite, and executes installed documentation examples. External
Clang/LLD/Z3 versions are checked; they are not hermetically vendored. A complete
LSP server/editor extension and external CI activation are not claimed.

**Languages/formats:** Rust compiler/CLI and editor adapter; Markdown reference and agent docs; TOML/YAML configuration. **Done when:** a clean locked build produces a version-matched installable candidate, docs run against that version, and basic editor diagnostics/navigation work on the shared semantic API.

### 30. Re-run fair benchmarks and make the alpha decision

**Status:** Runtime remeasurement and an explicit **narrow** decision complete;
the full fair agent experiment remains open. Checked-in five-sample Go/Rust and
Pactern reports retain raw timings, variance, environment, outcomes, and artifact
information. Pactern has an actual guarded/unguarded runtime ablation. Matched
model/budget/documentation agent runs, text-versus-semantic-edit ablations,
token/currency costs, held-out success rates, and review studies are unmeasured.
No productivity advantage or broad alpha-readiness claim is made. See the
[candidate evaluation](docs/alpha-evaluation.md).

**Languages/formats:** Rust harness; Pactern runs; Go/Rust comparisons; JSON/CSV results and Markdown report. **Done when:** Pactern and Go/Rust runs include text-versus-semantic-edit and with-versus-without-contract ablations, under matched models, budgets, documentation, and tool costs. Success, cost, time, variance, sample counts, and runtime metrics are published, followed by an explicit continue, narrow, or redirect decision against the stated targets.
