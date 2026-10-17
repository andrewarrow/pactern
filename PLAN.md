# Pactern

> Agents write. You set the terms.

**Vision and implementation plan — October 17, 2026.**

Pactern is a compiled programming language and toolchain for software built and
maintained by AI agents under human-owned contracts. People define the required
behavior and permitted authority. Agents implement changes. The compiler and
runtime produce inspectable evidence about whether those changes respect the
agreement.

The ambition is to make correct changes cheaper to produce, easier to review,
and safer to execute. “The best of them all” means demonstrating that outcome,
not accumulating every feature of the five languages studied here.

The repository now contains the milestone 2 Rust workspace and a CLI scaffold
for version and help output. The compiler, runtime, and project commands are
not implemented; most capabilities below remain design targets. Research
describes upstream documentation inspected on October 17, 2026; the upstream
implementations were not independently built or audited.

## 1. The opportunity

The README identifies the right shift: an agent often authors the implementation,
while a person remains responsible for the result. That makes the unit of
productivity a **correct, reviewable change**, rather than generated lines of code.

Pactern should close this loop:

```text
Owner's requirements and grants
              |
              v
Agent inspects a typed project and proposes a change
              |
              v
Compiler checks the change against the existing pact
              |
       +------+------+
       |             |
 Counterexample   Accepted implementation
 or diagnostic       + evidence
       |             |
       +-- repair    v
                  Bounded execution
```

The distinguishing product is the complete agreement between specification,
editing, verification, and execution. Contracts, effect systems, semantic queries,
and compiler diagnostics already exist elsewhere. Pactern must make them compose
reliably and demonstrate that the resulting workflow is worth adopting.

Start with **sandboxed backend components**: business rules, validation, data
transformations, and bounded jobs called by an existing application. A Mainbrella
integration could be an early consumer, but the language and host interface should
remain usable independently.

The longer-term destination is a general-purpose language for reliable services
and systems components. Kernels, hard real-time software, GPU programming, and a
complete web framework are outside the first release.

## 2. What to take from the five languages

These are complementary experiments with different maturity and goals. The
recommendations in the last column are Pactern design judgments.

| Language | Documented strengths and boundaries | What Pactern should take |
| --- | --- | --- |
| **MoonBit** | A practical typed language with managed memory, multiple targets, integrated tests, compiler-backed agent queries, and experimental formal verification. Its proof documentation currently models mathematical integers without establishing machine-overflow safety; async support varies by backend. [Agent tooling](https://docs.moonbitlang.com/en/stable/toolchain/moonide/index.html), [verification](https://docs.moonbitlang.com/en/stable/language/verification.html), [async](https://docs.moonbitlang.com/en/stable/language/async-experimental.html) | Familiar syntax, local inference, fast feedback, coherent packages/docs/tests, and structured task lifetimes. Specify arithmetic and target support precisely. |
| **Zerolang** | An experimental graph-native language. The semantic graph is authoritative; readable source is a projection. Queries and checked patches use graph identities and revision preconditions. Projection reconciliation and target coverage remain significant engineering concerns. [Graph architecture](https://zerolang.ai/concepts/graph-architecture), [projections](https://zerolang.ai/concepts/projections), [compile path](https://zerolang.ai/concepts/compile-path) | Stable identities, semantic queries, atomic edits, and stale-change rejection. Begin with text as the durable implementation source and a rebuildable semantic index; measure graph-native persistence before adopting it. |
| **Vera** | Mandatory contracts, explicit effects, typed positional references, Z3 verification, and supported runtime fallbacks. Agent-facing LSP methods already offer speculative edits and proof deltas. Its documentation distinguishes shipped checks from proposed verification tiers and records proof/code-generation mismatches and runtime-guard gaps. [Design](https://github.com/aallan/vera/blob/main/DESIGN.md), [LSP](https://github.com/aallan/vera/blob/main/LSP_SERVER.md), [known issues](https://github.com/aallan/vera/blob/main/KNOWN_ISSUES.md) | Contracts as compiler inputs, concrete counterexamples, and obligation-level change reports. Keep readable names and protect specifications separately from implementations. |
| **NanoLang** | Colocated shadow tests, a C compilation path, and a bytecode VM. Current documentation explicitly says shadow-test enforcement is not universal. NanoCore has machine-checked results, whose scope excludes correspondence with the production compiler and VM. [Repository](https://github.com/jordanhubbard/nanolang), [formal scope](https://github.com/jordanhubbard/nanolang/blob/main/formal/README.md), [release contract](https://github.com/jordanhubbard/nanolang/blob/main/docs/RELEASE_5.0.md) | Executable examples near code, dependency-aware testing, and a small core with explicit proof boundaries. Require meaningful evidence at important interfaces; avoid ceremonial tests on every helper. |
| **AILANG** | An immutable functional core with inferred types, explicit effects, runtime capabilities and budgets, contracts, and agent integration. Verification covers a supported fragment; recursive checks can be bounded. Trace replay currently means re-execution and comparison. [Types](https://ailang.sunholo.com/docs/architecture/types), [effects](https://ailang.sunholo.com/docs/reference/effects), [contracts](https://ailang.sunholo.com/docs/guides/contracts), [traces](https://ailang.sunholo.com/docs/guides/traces) | A pure core, visible effects, scoped authority, budget accounting, version-matched agent guidance, and measured agent evaluations. Make recorded replay suppress real external effects. |

Two qualifications to the README matter:

- MoonBit's October 10 agent article exists, but broad platform claims should be
  read alongside its detailed backend and verification limitations.
  [Official article](https://www.moonbitlang.com/blog/building-agents-moonbit)
- The reported Zerolang 0/60 result comes from a narrow May 2026 cold-generation
  experiment. Syntax and workflows have since changed. It motivates testing
  learnability; it does not establish a current ranking.
  [Original benchmark report](https://github.com/vercel-labs/zerolang/issues/104)

## 3. The pact is the central abstraction

A **pact** is a versioned agreement associated with a public boundary or important
invariant. It connects:

- **Behavior:** input assumptions, output guarantees, invariants, and accepted
  error cases.
- **Authority:** the effects an implementation may request and the resources the
  deployment may grant.
- **Evidence:** the proofs, runtime guards, examples, properties, and acceptance
  tests required for a change to be accepted.

These belong to different layers. A type describes a value; a contract constrains
behavior; an effect describes an operation; a capability authorizes access; a test
exercises selected cases. The toolchain connects them without treating one as a
substitute for another.

### Requirements survive repairs

An implementation agent must not make a failing change pass by deleting a test,
weakening a guarantee, strengthening an input assumption, widening a grant, or
lowering the required assurance level.

Protected contracts and acceptance tests live in reviewable files with stable
identities. An implementation transaction cannot alter them. A specification
transaction reports their exact changes for the owner's normal review process.
New tests and stronger candidate contracts may be proposed without replacing the
accepted baseline.

Protect the definitions and fixtures that determine a specification's meaning,
including assertion helpers and imported predicates. Initial contract expressions
may call only compiler primitives or protected pure helpers. Keep acceptance
oracles independent of the implementation being tested; changing the expected
answer indirectly is still a specification change.

Enforce this distinction in CI against a trusted base revision and externally
configured policy. A manifest editable by the same agent is not an authority
boundary. Direct text edits, renames, deleted symbols, and changed import paths
must pass the same gate as semantic edits. Approval is tied to the exact diff and
baseline; it does not transfer to a later changed proposal.
The acceptance checker and evidence-producing tools also come from trusted CI
configuration, rather than candidate-modified scripts.

This is more than preserving the count of passing proofs. Vera's edit protocol,
for example, explicitly treats deleting an obligation differently from losing an
existing proof. Pactern must make removal of a protected obligation an agreement
change. [Vera edit protocol](https://github.com/aallan/vera/blob/main/LSP_SERVER.md)

## 4. Language choices

### Familiar source, a small semantic core

Use conventional named functions, braces, `let`, `if`, `match`, and explicit
`Result` errors. Keep one canonical formatter and avoid alternate syntaxes for
the same construct.

The initial core should include:

- Booleans, checked fixed-width integers, UTF-8 strings, lists, records, tagged
  unions, and exhaustive matching.
- `Option` and `Result`, explicit conversions, local type inference, and declared
  public signatures.
- Modules and simple parametric generics, without specialization or complex
  trait resolution.
- Immutable values by default, explicit local mutation, ordinary control flow,
  and contracts that are pure expressions.

Specify overflow, division, indexing, evaluation order, and failure behavior
before optimizing. Start with checked `I64`; represent its actual range in the
verifier. Separate ordinary recoverable errors from contract violations and
resource-limit traps. Floating point and wrapping arithmetic wait until their
semantics and verification boundaries are explicit.

Exclude implicit numeric coercions, inheritance, user-defined operator
precedence, unrestricted macros, reflection over mutable program structure, and
ambient global state from the initial language. These are scope decisions,
not claims that such features are inherently wrong.

### Safe memory with honest costs

Choose managed ordinary values for v0.1, with a small tracing collector and precise
roots generated from the typed representation. This favors simple authoring and
safe composition. Measure allocation volume, retained memory, and collection
pauses. Do not promise C's memory footprint or hard real-time latency.

External resources use noncopyable handles with explicit scope-based cleanup;
the collector does not determine when a transaction, file, or capability closes.
The first implementation can restrict handle movement and capture rather than
introduce a general borrow checker. No raw pointers, finalizers, or arbitrary
in-process FFI in the safe subset.

Value layouts, stack allocation, and allocation elimination can improve later
without exposing lifetime syntax to every application. General ownership and
manual-memory features require a separate demonstrated use case.

### Pure computation, explicit effects

Public functions declare an upper bound on their effects. Omitted effects mean
pure; local inference may explain effects but cannot silently enlarge a public
boundary. Effects must propagate through calls and imports. Higher-order functions
must preserve those bounds before they enter the supported language.

An effect such as `db.write` is descriptive. Authority comes from an opaque handle
issued by the host, scoped to the allowed database operation and tenant or resource.
A callee can receive less authority than its caller, never manufacture more.
Host functions validate the handle and resource scope on every external operation.

Time, randomness, environment access, networking, storage, and model calls are
effects. AI inference is an optional library capability, not a compiler dependency
or a privileged language oracle. Returned model data must pass ordinary parsing,
typing, and application validation.

Start with sequential bounded invocations. Add structured concurrency after the
core: child tasks have bounded lifetimes, cancellation runs cleanup, shared
mutation is restricted, and child work charges ancestor budgets. Concurrency
needs its own semantics and tests before broad safety claims.

## 5. Source files and semantic changes

Keep readable `.ptn` files as the authoritative implementation representation.
The compiler builds a semantic database for symbols, types, effects, contracts,
callers, and dependencies. Ordinary editors, Git diffs, and offline builds remain
first-class workflows.

Use a tracked identity ledger for declaration and pact IDs. IDs survive supported
renames and moves; content hashes identify revisions, not identity. The ledger
stores identity only, not a second copy of program semantics. Text edits are
reconciled conservatively: ambiguous identity changes require explicit mapping,
and loss of a protected identity is an agreement change. The index can always be
rebuilt from source, contract files, manifests, and the ledger.

All interfaces use the same compiler library. The CLI is the first client;
editor and agent-protocol adapters follow after the API stabilizes.

Proposed commands:

```sh
pactern inspect quota.reserve --json
pactern callers quota.reserve --json
pactern impact --change change.json --json
pactern apply change.json --dry-run --json
pactern apply change.json --json
pactern verify --json
pactern test --affected --json
pactern build --target wasm
```

A semantic change names its base revision, target IDs, expected signatures or
hashes, and operations such as replacing a body or renaming a declaration. It is
validated against an isolated candidate workspace before application. Commit is
all-or-nothing, with the revision rechecked at commit time.

The result includes the source diff, affected interfaces, changed obligations,
new effects, executed checks, and remaining unknowns. Multi-agent edits use the
same optimistic concurrency checks; incompatible proposals return a conflict
instead of overwriting each other.

Queries return focused context with explicit truncation and completeness metadata.
Affected-test selection is conservative and falls back to broader checks when
dependency information is incomplete. Release CI still runs the full required
suite; a quick local repair loop does not replace acceptance checks.

Diagnostics have versioned JSON schemas, stable codes, source locations, symbol
and obligation IDs, and concrete counterexamples when available. Suggested edits
remain proposals and must pass the same checks when applied. Ship a concise
language reference and runnable examples matched to the installed compiler.

## 6. Verification that says exactly what it established

Generate executable checks and proof obligations from one typed core
representation. Do not independently reconstruct the meaning of arithmetic,
contracts, or types in several tools.

Track assurance per obligation, with independent dimensions:

| Dimension | Reported result | Meaning |
| --- | --- | --- |
| Static verification | `proved`, `refuted`, `unknown`, `unsupported` | Proof applies to the stated model and assumptions. A timeout is `unknown`. |
| Bounded exploration | `pass_within_bound`, `counterexample`, `not_run` | Records the explored input, recursion, or state bound; it is not an unbounded proof. |
| Runtime enforcement | `guarded`, `unguarded` | A guarded obligation maps to an actual emitted check in this artifact. |
| Executed tests | `pass`, `fail`, `not_run` | Records examples, property seeds, and execution environment. Passing tests are not proofs. |

A project policy decides what evidence each protected pact requires. Critical
pure invariants can require static proof. Supported dynamic conditions may permit
runtime enforcement. Unsupported checks, timeouts, and skipped tests never
silently satisfy a proof requirement.
Refuted obligations and failing required tests reject a candidate. An unproved,
unguarded fact cannot become an assumed guarantee for downstream proofs.

Begin with quantifier-free arithmetic and Boolean contracts over pure functions.
The initial proof fragment is loop-free with an acyclic call graph; add records
and tagged unions incrementally. Generate proof obligations for possible
checked-arithmetic and bounds traps. Proof reports distinguish postcondition
validity, absence of traps, and termination; a postcondition alone does not
prove that a result will be returned. Execution failure never counts as an
acceptance-test success.
Recursive proofs, heap reasoning,
concurrent invariants, and theorem-prover integration are later work. Reject or
flag inconsistent assumptions and vacuous contracts; do not celebrate a theorem
whose preconditions can never hold.

Runtime checks do not undo effects that have already occurred. Validate inputs
before invoking external operations. Persistent invariants also need transactional
host APIs and storage constraints; a postcondition on a helper cannot establish
database isolation or distributed exactly-once execution.

Evidence records bind to source and dependency hashes, target, compiler version,
contract and policy revisions, solver configuration, assumptions, and artifact
hash. Invalidate cached evidence when any relevant input changes. A proof remains
conditional on the compiler, solver, runtime, and trusted host; producing this
record does not make the whole toolchain formally verified.

## 7. Execution and compiler architecture

Implement the initial toolchain in Rust. Keep a small reference interpreter as a
semantic oracle, then lower the same typed core to C and compile that to WebAssembly
for the first deployment target. Use a narrow, versioned host ABI and one supported
Wasm host initially. This avoids maintaining several production backends at once.

```text
.ptn source + pacts + identities
                |
          parser / resolver
                |
         typed core + effects
          /      |       \
  semantic API   |    obligations + guards
                 |
       interpreter / C lowering
                           |
                     WebAssembly
                           |
                capability-checking host
```

The C backend must preserve checked arithmetic, evaluation order, bounds checks,
and GC roots without relying on undefined C behavior. Differential tests compare
the interpreter and compiled execution on generated programs, including errors
and traps. Fuzz the parser, type checker, lowering, and runtime boundaries.

WebAssembly is an isolation mechanism, not the permission policy by itself. The
host grants no ambient filesystem, network, environment, or process authority.
It validates imports and handles and enforces memory, execution, and effect
budgets. Host operations need their own timeouts and size limits; guest instruction
limits do not bound a blocking database call.
The host owns rollback and handle revocation on traps, timeouts, and cancellation;
guest cleanup cannot be relied on after execution fuel has run out.

Budget accounting covers nested calls and, eventually, child tasks. Track
operation count, bytes, elapsed time, memory, and model spending separately.
AILANG's ancestor-charged effect budgets are a useful starting point for the
accounting model. [Capability budgets](https://ailang.sunholo.com/docs/reference/capability-budgets)

Record external requests and results at the host boundary. **Replay substitutes
recorded results and never performs real external writes.** It rejects mismatched
requests and incompatible artifact or policy revisions. Missing or redacted data
means replay is unavailable, not permission to call the live service. Keep live
re-execution a separate operation. Full traces may contain sensitive inputs, so
capture and retention are explicit host settings.

Native output through the C backend can follow once semantics and host behavior
are tested. Native code requires separate OS isolation for untrusted execution;
C interoperability alone supplies no such boundary. Initially, existing services
call a Pactern Wasm component. Broader FFI belongs behind audited adapters or
isolated processes.

## 8. The first convincing demonstration

Build a **tenant-scoped job quota component** that reserves capacity without
producing a negative balance. This exercises ordinary business logic, authorization,
storage, and agent repair without requiring a complete application framework.

The pure core could look like this. Syntax is illustrative, not a frozen grammar;
the protected pact would live in a separate contract file.

```text
enum ReserveError { InvalidAmount, InsufficientQuota }

pub fn reserve(available: I64, amount: I64) -> Result<I64, ReserveError> {
    if amount <= 0 {
        return Err(InvalidAmount)
    }
    if amount > available {
        return Err(InsufficientQuota)
    }
    return Ok(available - amount)
}

pact reserve_safety for reserve {
    requires available >= 0
    ensures match result {
        Ok(remaining) =>
            amount > 0 && amount <= available &&
            remaining == available - amount && remaining >= 0,
        Err(InvalidAmount) => amount <= 0,
        Err(InsufficientQuota) => amount > 0 && amount > available,
    }
}
```

The effectful wrapper receives a host-issued tenant quota handle. It reads
`available` from that tenant's storage inside the same transaction that writes
the new value; the caller supplies only the requested amount. Validate the stored
value against the precondition, use serializable execution or an equivalent
conditional update, and enforce a nonnegative storage constraint. Authentication,
atomicity, and retry behavior are separate tested parts of that wrapper.

The demo is complete when an agent can:

1. Inspect only the relevant signature, pact, callers, and examples.
2. Introduce a faulty implementation and receive a useful counterexample, such
   as reserving four units from three.
3. Repair the implementation while keeping the protected pact and tests intact.
4. Have an attempted pact deletion or permission expansion rejected by the
   external acceptance policy.
5. Have stale concurrent edits rejected without changing the workspace.
6. Execute the compiled component with access to exactly one tenant's quota,
   with cross-tenant writes denied and concurrent reservations tested.
7. Reproduce a recorded failing invocation without performing storage writes.
8. Produce an artifact-linked evidence report and a concise reviewable diff.

Pure arithmetic proof covers the helper. Transaction tests, host authorization
checks, and storage constraints provide separate evidence for the service.

## 9. Build in milestones with exit gates

No calendar estimates until the compiler and runtime spikes establish costs.
Each milestone should leave a usable artifact and a reason to continue.

| Milestone | Deliverable | Exit gate |
| --- | --- | --- |
| **0 — Specify and measure** | A compact grammar, core semantics, pact/diagnostic schemas, threat boundary, and an initial held-out task harness. Build a strong Go or Rust tooling baseline. | Representative programs and failure cases have unambiguous expected behavior; the baseline can be reproduced. |
| **1 — Execute a small language** | Parser, resolver, type checker, formatter, modules, reference interpreter, minimal managed runtime, and C-to-Wasm path. Begin with pure code and fixture inputs. | `reserve` and data-transformation examples run consistently; overflow, bounds, malformed inputs, and memory limits fail as specified. |
| **2 — Establish the pact** | Contract files, examples, property tests, supported runtime guards, a narrow SMT integration, and an obligation inventory. | The quota defect yields a counterexample; unsupported proof cases remain explicit; every advertised guard is present in the compiled artifact. |
| **3 — Make changes transactional** | Semantic queries, identity reconciliation, versioned JSON, atomic patches, impact analysis, and protected-baseline CI checks. | Rename/move and stale-edit tests pass; agents repair implementations without deleting requirements; direct file edits cannot bypass the same gate. |
| **4 — Bound useful effects** | Scoped host handles, one transactional storage adapter, budgets, recorded replay, and the quota wrapper. | Cross-tenant and budget escape tests pass; failure/cancellation cannot leak authority or leave partial transactions; replay performs zero live writes. |
| **5 — Evaluate and publish an alpha** | Installable toolchain, editor basics, executable agent reference, examples, locked dependencies, reproducible builds, and published benchmark artifacts. | Independent users complete the demo; declared target/runtime support is tested; evaluation supports continuing with a new language. |

The v0.1 scope ends here. Defer structured concurrency, user-defined higher-order
effects, broad native FFI, additional backends, advanced proof systems, self-hosting,
and a package registry. Existing package sources can be pinned without operating
a registry. Builds and dependency tests run with explicit grants; packages do not
acquire authority merely by being imported.

Before calling the toolchain stable, add upgrade and compatibility tests for
source, package metadata, diagnostic schemas, identity data, and evidence records.
Keep the language specification and conformance suite reviewable under the
repository's existing Apache-2.0 direction.

## 10. Define “better” before claiming it

Evaluate both small new programs and changes to existing projects: renames,
schema changes, bug fixes, permission changes, unfamiliar dependencies, and
multi-file regressions. Include adversarial repair attempts that modify tests or
weaken contracts. Keep acceptance tests hidden from the implementation agent.

Use the same model versions, tasks, compute budgets, and access to documentation.
Provide idiomatic tooling to every baseline and count language-teaching context
and tool responses in token costs. Separate first-attempt, fixed-budget repair,
and full agentic results; report failures, variance, and sample counts.

Compare against strong Go and Rust workflows first. Include MoonBit and the other
researched languages on tasks they support, documenting exclusions. Add ablations:
Pactern text-only versus semantic editing, with and without contracts, and an
existing-language implementation of similar agent tooling. This tests whether
the benefit requires a new language.

The README's figures become hypotheses, not launch claims:

| Measure | Proposed continuation target |
| --- | --- |
| Correct accepted changes within a fixed budget | At least baseline success, with a meaningful improvement on contract- and permission-sensitive tasks. |
| Total tokens per correctly completed task | Test the README's 30% reduction target, including documentation and failed attempts. |
| Compile/repair cycles | Test the README's 50% reduction target; also report total wall time. |
| Unauthorized effects and specification tampering | Zero successful escapes in the published adversarial suite, without implying universal security. |
| Human review | Fewer missed requirement changes and lower review time, measured with reviewers rather than inferred from diff size alone. |
| Runtime cost | Publish startup, throughput, p95/p99 latency, peak memory, artifact size, and host overhead on fixed hardware. Establish workload-specific budgets before promising parity with Go. |

Publish raw task definitions where possible, prompts, tool versions, run logs,
and unsuccessful runs. AILANG's published self-audit is a useful example of testing
an agent-language thesis against a conventional-language baseline, including
harness problems. [Self-audit](https://ailang.sunholo.com/docs/guides/three-camps-self-audit)

If most gains come from semantic tooling and protected contracts that work equally
well on Go or Rust, redirect the project toward that tooling. If proof costs swamp
the repair loop, narrow the supported fragment. If runtime costs miss the initial
application niche, revise the memory or host design before expanding scope.

## 11. Immediate next work

1. Turn the decisions above into a short core-language specification with exact
   integer, error, effect, handle, and contract semantics.
2. Write the quota example, two ordinary transformation examples, and their
   expected successes and failures as conformance fixtures.
3. Define the first JSON diagnostic and semantic-change schemas, including stale
   revisions and protected-obligation changes.
4. Establish the Go/Rust baseline on those tasks before teaching models Pactern.
5. Build the smallest Rust parser, type checker, and interpreter needed to run
   the pure quota example; add C/Wasm lowering only against that semantic oracle.

Pactern earns its place when an agent can make a useful change, explain the evidence,
and leave the owner's agreement intact—at a lower total cost than the best
existing workflow.
