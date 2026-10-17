# Planned implementation milestones

These are sequential planned milestones; each has an observable exit criterion. Work may proceed in parallel only where prerequisites are ready. See [PLAN.md](PLAN.md) for the design rationale and v0.1 scope.

Rust is the compiler, tooling, and sandbox-host language. The Rust compiler lowers Pactern to generated C, which is compiled to WebAssembly; a small C guest runtime supplies the runtime ABI and garbage collection. SMT support means Rust emits SMT-LIB for an external solver. Markdown, EBNF, JSON, TOML, and YAML below are data or documentation formats, not language implementations. Nothing in this roadmap is claimed complete.

These 30 v0.1 milestones exclude compiler self-hosting, general concurrency, broad FFI, and a production native backend.

## Specify and measure

### 1. Specify the first language and protocol contracts

**Languages/formats:** Markdown specification, EBNF grammar, JSON Schema documents. No code implementation. **Done when:** the versioned core semantics, diagnostic and change-request schemas, effect boundary, and supported subset are reviewable, with examples resolving edge cases for integers, errors, handles, and contracts.

### 2. Establish the Rust workspace and CLI skeleton

**Languages/formats:** Rust; TOML manifests. **Done when:** a clean checkout builds an installable `pactern` CLI and library with versioned command and JSON-output entry points.

### 3. Seed conformance fixtures and baseline tasks

**Languages/formats:** Pactern fixtures; Rust baseline harness; Go and Rust comparison programs; JSON task data. **Done when:** fixture inputs have recorded expected values, diagnostics, traps, and pact outcomes, and the Rust harness reproduces Go/Rust baseline results and costs before Pactern execution exists.

## Build the language path

### 4. Implement source spans, lexer, and parser

**Languages/formats:** Rust; Pactern source and EBNF grammar. **Done when:** valid fixture programs produce syntax trees with byte and line/column spans, while malformed and fuzz-generated input returns bounded, located diagnostics without crashing.

### 5. Resolve modules and names

**Languages/formats:** Rust; Pactern modules. **Done when:** imports, visibility, shadowing, duplicate names, and unresolved names match fixtures and diagnostics point to the relevant declarations and uses.

### 6. Type-check the core language

**Languages/formats:** Rust; Pactern source. **Done when:** fixtures establish typing for `Bool`, checked `I64`, UTF-8 strings, lists, records, tagged unions, `Option`/`Result`, and exhaustive matching, plus local inference and explicit public signatures. The checker accepts immutable-by-default bindings and explicit local mutation, and rejects examples that violate these rules.

### 7. Add simple generic functions and types

**Languages/formats:** Rust; Pactern source. **Done when:** the documented generic cases type-check and unsupported inference or specialization cases receive explicit diagnostics rather than silently changing types.

### 8. Define a typed core IR

**Languages/formats:** Rust; serialized debug IR as JSON where useful. **Done when:** parsed and resolved programs lower into a typed representation that records source origins and has validated invariants for every supported construct.

### 9. Build the pure reference interpreter

**Languages/formats:** Rust; Pactern programs. **Done when:** the quota helper and transformation fixtures execute from typed IR with specified values, errors, checked-overflow traps, and evaluation order.

### 10. Add formatting and structured diagnostics

**Languages/formats:** Rust; Pactern source; JSON diagnostic schema. **Done when:** formatting is stable under repeated runs and CLI diagnostics conform to the versioned schema with stable codes, spans, and relevant symbol IDs.

### 11. Track effects and noncopyable handles

**Languages/formats:** Rust; Pactern source. **Done when:** static checks propagate effects through calls and imports, reject effects that exceed a declaration's public bounds, and reject copying, escaping, or using scoped handles after their permitted lifetime in supported cases.

## Lower to a sandboxed executable

### 12. Implement the C guest runtime and ABI

**Languages/formats:** C guest runtime; Rust ABI definitions and test driver. **Done when:** runtime tests demonstrate allocation, precise root tracking and collection, value representation, checked traps, and cleanup behavior under the documented ABI.

### 13. Emit C from typed core

**Languages/formats:** Rust C emitter; generated C output. **Done when:** supported pure fixtures compile as generated C and agree with the interpreter for values, evaluation order, arithmetic traps, bounds checks, and GC roots.

### 14. Compile C to Wasm and add a minimal Rust host

**Languages/formats:** Rust build and sandbox host; generated C; WebAssembly target. **Done when:** a built component runs under a host that denies all ambient imports by default and enforces memory and execution limits, with rejected imports and limit traps covered by runtime checks.

### 15. Establish continuous differential and fuzz validation

**Languages/formats:** Rust harnesses and compiler; Pactern generated inputs; C/Wasm artifacts. **Done when:** routine validation compares interpreter and Wasm outcomes, fuzzes parser/type/lowering/runtime boundaries, and preserves minimized regressions for every discovered mismatch.

## Check pacts and produce evidence

### 16. Parse pacts and inventory obligations

**Languages/formats:** Rust; Pactern contract files; JSON reports. **Done when:** contracts attach to resolved declarations and produce revision-scoped obligation IDs with explicit supported, unsupported, and inconsistent cases.

### 17. Add example and property execution

**Languages/formats:** Rust runner; Pactern examples and properties; JSON seeds/results. **Done when:** deterministic examples and seeded properties execute against interpreter and compiled forms, recording failures, seeds, and environment.

### 18. Emit runtime contract guards

**Languages/formats:** Rust compiler; generated C and Wasm; Pactern contracts. **Done when:** each advertised runtime-enforced obligation maps to a check in the artifact, and deliberately violated inputs fail with the expected guard result.

### 19. Integrate narrow SMT verification

**Languages/formats:** Rust obligation generator; SMT-LIB bit-vector and datatype encodings; external solver. **Done when:** exact checked machine-`I64` semantics, including overflow and other specified traps, are encoded for loop-free pure code; supported enums and `Option`/`Result` suffice to verify the quota example. Reports separately state postcondition validity, absence of traps, and termination; bounded exploration is labelled with its bound and remains distinct from unbounded proof, while timeout and unsupported cases remain unknown or unsupported.

### 20. Bind evidence to artifacts and policy

**Languages/formats:** Rust; JSON evidence schema. **Done when:** reports bind results to source/dependency hashes, compiler and solver versions, assumptions, policy, target, and artifact hash, and relevant changes invalidate cached evidence. Proof, runtime guard, and executed-test results remain separate; refuted obligations or failed required tests reject a candidate, and unknown or unsupported results cannot satisfy required proof.

## Make semantic changes reviewable and protected

### 21. Assign persistent IDs and build a semantic index

**Languages/formats:** Rust; JSON identity data and rebuildable semantic index. **Done when:** declarations and pacts retain persistent IDs across supported rename and move edits, the index can be rebuilt from source and identity data, and ambiguity is surfaced for review rather than guessed silently.

### 22. Add inspection, impact, and affected-test queries

**Languages/formats:** Rust library and CLI; JSON request/response schemas. **Done when:** queries return focused, completeness-marked context, affected interfaces and obligations, and conservatively broaden test selection when dependency data is incomplete.

### 23. Apply revision-checked edits atomically

**Languages/formats:** Rust; JSON semantic-change requests. **Done when:** candidate edits validate against a base revision and commit only if the revision still matches; stale or invalid edits leave all workspace files unchanged.

### 24. Protect specs and enforce an external CI baseline

**Languages/formats:** Trusted Rust acceptance tools outside candidate changes; protected specifications, helpers, fixtures, and oracles; CI configuration (YAML). **Done when:** policy checks reject direct-text-edit bypasses and semantic changes that weaken protected requirements; approved spec changes are bound to an exact baseline and diff, with the trusted acceptance tools fixed outside candidate changes. Release CI runs the full required suite.

## Bound effects and demonstrate the workflow

### 25. Add scoped capabilities, budgets, and host cleanup

**Languages/formats:** Rust host; small C guest runtime; Pactern effect declarations; JSON policy. **Done when:** host-issued handles enforce scope and operation/byte/time/memory budgets, charge nested calls to the appropriate budget, and traps, cancellation, or timeout revoke authority and clean up host-owned resources.

### 26. Add a transactional storage adapter

**Languages/formats:** Rust adapter and tests; Pactern wrapper; SQL schema or equivalent storage constraints. **Done when:** tenant-scoped read/update uses one transaction or conditional update, storage rejects negative quota, and concurrency tests show no over-reservation or cross-tenant access.

### 27. Record and replay host interactions

**Languages/formats:** Rust host; JSON or binary trace format. **Done when:** replay substitutes recorded results, rejects request or revision mismatches, fails when required recorded data is missing or redacted, and tests verify replay performs zero live external writes.

### 28. Complete the quota demonstration

**Languages/formats:** Pactern implementation, protected pact, and examples; Rust tooling/host; C runtime and Wasm output. **Done when:** a scripted agent workflow inspects relevant context, exposes and repairs a quota defect, cannot weaken protected rules, rejects stale edits, runs tenant-isolated transactional code, replays a failure without writes, and emits artifact-linked evidence.

## Prepare and decide on an alpha

### 29. Package an editor-ready reproducible candidate

**Languages/formats:** Rust compiler/CLI and editor adapter; Markdown reference and agent docs; TOML/YAML configuration. **Done when:** a clean locked build produces a version-matched installable candidate, docs run against that version, and basic editor diagnostics/navigation work on the shared semantic API.

### 30. Re-run fair benchmarks and make the alpha decision

**Languages/formats:** Rust harness; Pactern runs; Go/Rust comparisons; JSON/CSV results and Markdown report. **Done when:** Pactern and Go/Rust runs include text-versus-semantic-edit and with-versus-without-contract ablations, under matched models, budgets, documentation, and tool costs. Success, cost, time, variance, sample counts, and runtime metrics are published, followed by an explicit continue, narrow, or redirect decision against the stated targets.
