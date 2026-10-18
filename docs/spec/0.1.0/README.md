# Pactern specification 0.1.0

**Status:** unreleased draft, revision 2. This bundle defines the candidate
language and protocol; [MILESTONES.md](../../../MILESTONES.md) separately records
the experimental implementation and remaining release gates. Revision 2 adds the
1,000-line source limit and the semantic/editor query contracts.

The [vision](../../../PLAN.md) explains the design. This bundle makes the first
language subset and wire contracts concrete enough to implement and review.

| Document | Purpose |
| --- | --- |
| [Language](language.md) | Core types, evaluation, errors, effects, handle ownership, contracts, and proof boundary |
| [Grammar](grammar.ebnf) | Lexical rules and token-level EBNF for the language |
| [Examples](examples.md) | Expected successes, static errors, traps, and verification outcomes |
| [Protocol](protocol.md) | Versioning, diagnostics, revisions, semantic edits, and acceptance boundaries |
| [Diagnostic schema](schemas/diagnostic.schema.json) | Machine-readable diagnostic shape |
| [Change-request schema](schemas/change-request.schema.json) | Machine-readable candidate edit shape |
| [Semantic query request](schemas/semantic-query-request.schema.json) / [response](schemas/semantic-query-response.schema.json) | Inspection, callers, impact, and affected tests |
| [Editor request](schemas/editor-request.schema.json) / [response](schemas/editor-response.schema.json) | Diagnostics and declaration navigation |

Protocol example payloads and their expected validation outcomes are described in
[the protocol document](protocol.md). JSON Schema validates structure; contextual
rules such as a matching revision, valid types, protected identity, and authorization
remain explicit service obligations.

## Decisions resolved in this draft

- Statements have explicit semicolons; `if`, `while`, and `match` use parenthesized
  inputs. Examples in the earlier vision were illustrative, not a fixed grammar.
- All function signatures are explicit; omitted effects mean pure. Operators and
  conversions have a fixed, small set of meanings.
- `I64` overflow traps in every build mode. Division and remainder specify the
  minimum-integer edge case, evaluation order, and short-circuit behavior.
- Ordinary values are immutable and managed. Only local binding replacement is
  mutable. Host handles move or borrow under a restricted synchronous model.
- Pacts live in separate contract files; their meaning-defining dependencies are
  protected along with the declarations themselves.
- Runtime guards remain enabled in the initial execution model. Static proofs
  have a declared subset and separate postcondition, trap, and termination claims.
- Semantic changes name a base revision, target identity, and expected content.
  JSON alone cannot authorize a specification change or widen permissions.

## Versioning and validation

The bundle version is `0.1.0`. Until the first public distribution, draft revisions
record normative changes in this candidate directory. After distribution, a
normative language or protocol change requires a new versioned bundle; editorial
corrections may remain. Tools select an exact supported wire version and reject
unknown versions rather than guess. There is no released compatibility promise
between these unreleased drafts.

The language semantics and grammar are normative together. Contradictions are
specification defects to fix before implementation, not permission to choose a
convenient interpretation. Examples illustrate the same rules. Future conformance
fixtures should cite the relevant rule and expected outcome.

Validation includes parser and type/effect conformance, interpreter/Wasm
comparisons, pact guards and narrow SMT claims, semantic transactions, schemas,
and the version-matched install workflow. See [development](../../development.md)
for reproducible commands. JSON Schema checks structure; trusted context and
execution checks remain necessary.
