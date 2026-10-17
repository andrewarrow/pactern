# Pactern specification 0.1.0

**Status:** first reviewable draft, revision 1. This bundle completes the
specification deliverables of [milestone 1](../../../MILESTONES.md). It does not
implement a compiler, runtime, verifier, or semantic-edit service.

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

The bundle version is `0.1.0`; the draft revision records this initial publication.
Any subsequent normative language or protocol change must publish a new versioned
bundle. Editorial corrections may remain in this directory. Tools must select an
exact supported version and reject unknown wire versions rather than guess.

The language semantics and grammar are normative together. Contradictions are
specification defects to fix before implementation, not permission to choose a
convenient interpretation. Examples illustrate the same rules. Future conformance
fixtures should cite the relevant rule and expected outcome.

Validation for this milestone checks the EBNF's internal structure, JSON Schema
definitions and example payloads, Markdown links, and cross-document consistency.
There is no Pactern parser yet, so source examples have been reviewed against the
grammar rather than executed. Milestones 2 and 3 establish the Rust workspace and
executable conformance/baseline harness respectively.
