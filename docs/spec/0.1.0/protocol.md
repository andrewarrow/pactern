# Pactern protocol 0.1.0

Status: draft design for milestone 1. This document specifies diagnostic messages
and semantic change requests. It does not claim that a compiler, change service,
approval service, or verifier exists. The [language specification](language.md)
defines source meaning; this protocol does not give JSON a second program meaning.

The schemas use [JSON Schema Draft 2020-12](https://json-schema.org/draft/2020-12/json-schema-core)
and its [validation vocabulary](https://json-schema.org/draft/2020-12/json-schema-validation).
They are self-contained and can be checked offline with a conforming validator:

- [Diagnostic schema](schemas/diagnostic.schema.json)
- [Change-request schema](schemas/change-request.schema.json)

## 1. Version and validation rules

Every message carries `protocol_version: "0.1.0"` and a `kind`. A consumer rejects
unknown versions, fields, operations, or discriminants; it does not silently
discard them. The schema URNs identify these local documents, not network services.
Future wire-format changes receive a new protocol version and separate schemas.
No compatibility with a future `0.1.1` is implied by accepting this draft.

Payloads are UTF-8 JSON with unique object keys and Unicode scalar strings.
Reject duplicate keys and unpaired surrogate escapes while decoding, before
schema validation; a decoder that silently replaces them is unsuitable. Object
key order is irrelevant. Schema validation checks structure. Name resolution,
source parsing, integer ranges, hashes, permissions, and snapshot consistency
are separate semantic checks. Structural validity never grants authority.

## 2. Identities, hashes, and source locations

Declaration IDs have the form `sym_` followed by 32 lowercase hexadecimal digits;
pact IDs use `pact_` with the same suffix length. IDs are opaque ledger entries,
not names, source positions, or hashes. They survive supported body edits and
renames. Allocation must avoid collisions and an ID must never be reused for an
unrelated declaration. Text-edit reconciliation must report ambiguous identity
matches instead of guessing. No source-level ID annotation is introduced here.

An obligation ID uses `obl_` plus 32 lowercase hexadecimal digits and is meaningful
only with its `source_revision`. Unlike a declaration ID, it is not promised to
survive a change in the verified obligation. Tools allocate these IDs when they
construct an obligation inventory; clients treat them as opaque.

A digest is `sha256:` followed by 64 lowercase hexadecimal digits. A target's
`expected_hash` hashes the exact UTF-8 bytes of the entire existing declaration,
starting at its first keyword (`pub` when present) and ending immediately after
its closing brace. Include internal whitespace and comments; exclude preceding
and trailing trivia. No newline or Unicode normalization is performed. A body
change therefore invalidates the declaration hash even if its signature remains
the same. A specification operation hashes its existing pact in the same way.

A span contains `path`, `start_byte`, and `end_byte`. Paths are relative to the
workspace root, use `/`, and have no empty, `.` or `..` components, leading or
trailing slash, backslash, colon, or control characters. They identify source in
the associated snapshot, not arbitrary files on the machine. Byte offsets are
zero-based and half-open: `[start_byte, end_byte)`. Both endpoints must be UTF-8
scalar boundaries, with `0 <= start_byte <= end_byte <= file length`; a zero-width
span is allowed. Offset numbers must be exact integers no greater than
`9007199254740991`. Larger source files are unsupported by this protocol version.

For `aéz`, byte offsets `[1, 3)` select `é`, while `[1, 2)` is invalid. The protocol
does not transmit line/column positions; editor clients derive their own display
or UTF-16 positions from the revision's source bytes. A missing or `null` primary
span denotes a diagnostic without a source location. A located diagnostic or
related location requires `source_revision`; consumers must not display its
offsets against different source bytes.

## 3. Workspace revisions and policy baselines

`base_revision` identifies the full input snapshot being edited. It is not a Git
commit ID: dirty and untracked participating files, identities, dependencies,
configuration, and policy all matter. A changed file, added input, removed input,
or different tool configuration must change the revision.

To make snapshot digests reproducible, define `H(tag, records)` as SHA-256 over:

1. The UTF-8 bytes `pactern/0.1.0/`, then `tag`, then one LF byte.
2. Each `(category, key, digest)` record, with unique `(category, key)` pairs,
   sorted by the unsigned UTF-8 bytes of `category`, then of `key`. Reject duplicate
   pairs before hashing, even when their digests agree. Write each string with its byte length as an
   unsigned 64-bit big-endian integer followed by its UTF-8 bytes. Write `digest`
   as its 32 raw SHA-256 bytes, without a length prefix or textual `sha256:` prefix.

Record strings contain only Unicode scalar values. A digest of file or external
input content is SHA-256 of its exact bytes. The resulting hash is rendered in
the digest form above. The workspace revision is `H("workspace", records)` with
the following inventory:

| Category | Key and required content |
| --- | --- |
| `file` | Canonical workspace-relative path and bytes of every participating source, pact, identity-ledger, manifest, lockfile, protected helper, oracle, and test fixture. Include untracked inputs. |
| `dependency` | Resolved dependency identifier and a verified content-root digest over its recursively resolved files and dependencies. A lockfile or remote name alone is insufficient. |
| `configuration` | Named semantic configuration input and exact bytes: target, protocol and language versions, solver settings, feature settings, semantic environment inputs, and ordered build/check arguments. These arguments use keys `argument/00000000`, `argument/00000001`, and so on, and hash the UTF-8 bytes of each argument. |
| `tool` | Tool identifier and digest of the pinned compiler, solver, and acceptance-tool distribution content, including their declared runtime dependencies. |
| `policy` | Key `baseline` and the trusted policy-baseline digest described below. |

Inspection, staging, and commit use the same trusted semantic configuration
profile. Exclude transport/control data from its arguments: command verbs,
request IDs, revision fields, operation payloads, dry-run flags, and presentation
or output paths. These do not change program meaning and must not make the hash
self-referential or make an inspection revision immediately stale on application.

The trusted host fixes the input inventory and records exact external input bytes
and content roots. Clients cannot exclude inconvenient files or configuration.
Dependency roots use `H("dependency", records)` over that dependency's `file` and
`dependency` entries; cyclic dependency roots are unsupported. Input discovery
includes all eligible files under configured source and test roots, so adding a
new eligible file changes the snapshot. Output directories and caches are
excluded by trusted configuration. Builds cannot read additional semantic inputs
after snapshot creation; undeclared reads fail instead of producing unbound
evidence. Filesystem timestamps and Git metadata are not semantic inputs.

`expected_policy_baseline` must equal the host's current trusted baseline, even
when the candidate contains a different policy file. It is `H("policy", records)`
over the externally selected policy configuration, protected identities and
accepted contents, protected dependency closure, required evidence rules, and
trusted acceptance-tool distributions. Use the same categories and digest rules,
omitting the `policy` category to avoid self-reference. Its manifest is owned
outside candidate edits. Candidate policy changes do not install a new baseline.

The binary framing, sorted records, exact bytes, and content-root rules above are
hash canonicalization. JSON serialization order and formatter output are not
hash canonicalization. A request obtains revision and target hashes from a
consistent tool snapshot; it must not manufacture them from Git HEAD.

## 4. Diagnostics

A diagnostic requires `code`, `severity`, `phase`, and `message`, in addition to
its version and kind. `message` is human-readable and may improve between tool
releases; consumers branch on stable `code`, not wording. Codes identify meaning
and must not be reassigned. Severity is `error`, `warning`, or `note`. Optional
symbol, pact, and obligation IDs link to the associated semantic inventory.
`related` supplies additional located explanations, and `notes` supplies plain
explanatory text. Diagnostics never contain automatically authorized edits.

The initial code registry is intentionally small:

| Code | Meaning |
| --- | --- |
| `PTN_PARSE_UNEXPECTED_TOKEN` | Source does not match the grammar. |
| `PTN_TYPE_MISMATCH` | A value or expression has an incompatible type. |
| `PTN_RUNTIME_INTEGER_OVERFLOW` | Checked arithmetic cannot represent its result. |
| `PTN_CONTRACT_PRECONDITION` | A runtime entry guard failed. |
| `PTN_VERIFY_REFUTED` | A supported static obligation has a countermodel. |
| `PTN_VERIFY_INCONSISTENT` | No input satisfies the preconditions without trapping. |
| `PTN_VERIFY_UNKNOWN` | Verification did not establish or refute the claim. |
| `PTN_VERIFY_UNSUPPORTED` | The obligation is outside the supported proof fragment. |
| `PTN_CHANGE_INVALID_REQUEST` | Decoding, schema, operation, or semantic preconditions failed. |
| `PTN_CHANGE_STALE_REVISION` | The requested base is no longer the current snapshot. |
| `PTN_CHANGE_STALE_TARGET` | A target declaration hash differs from the expected hash. |
| `PTN_CHANGE_POLICY_MISMATCH` | The external baseline differs from the requested baseline. |
| `PTN_CHANGE_PROTECTED_TARGET` | An implementation transaction changes protected meaning or content. |
| `PTN_CHANGE_APPROVAL_REQUIRED` | A specification candidate lacks matching external approval. |

The schema accepts the `PTN_<AREA>_<NAME>` code family so a producer can add codes
without changing this envelope. An unfamiliar valid code remains displayable;
it must not be interpreted as success. Phase names enumerate the producing
pipeline stage; no diagnostic alone establishes overall command success.

An optional `verification` object reports a single static claim:
`precondition_consistency`, `postcondition`, `absence_of_traps`, or `termination`,
with `proved`, `refuted`, `unknown`, or `unsupported` status. Consistency means at
least one input satisfies the preconditions without trapping; `refuted` consistency
means there is no such input. The object requires a revision and obligation ID
and appears only in the `verify` phase. A timeout is `unknown`. This is explanatory
output, not the artifact-linked evidence and policy decision planned for
milestone 20.

A counterexample records its origin (`solver_model`, `test_execution`, or
`bounded_exploration`), named bindings, and whether the candidate was actually
replayed. `replayed: false` does not claim an observed execution failure. Binding
names must be unique within the counterexample. A counterexample requires
`source_revision`, even without a source span or a static verification claim.
This draft carries scalar
`I64`, `Bool`, and `String` values; richer typed values require a later version.
Omit unsupported values and describe their omission in `notes`; never invent a
stringified representation that consumers could mistake for a typed value.

An `I64` is a canonical decimal **string**, from `"-9223372036854775808"` through
`"9223372036854775807"`. No leading zeros, plus sign, whitespace, `-0`, or exponent
is allowed. The schema enforces the string shape; a semantic check enforces the
exact signed 64-bit range. This preserves every integer across JSON clients that
would round a JSON number. Boolean and string values use their JSON equivalents.

A solver model is not a passing test, an executed test is not a proof, bounded
exploration is not an unbounded proof, and a postcondition result does not certify
trap freedom or termination. Acceptance must consult the complete required
evidence, not count favorable diagnostics.

## 5. Change requests

A request contains `request_id`, `transaction`, `base_revision`,
`expected_policy_baseline`, and a nonempty `operations` array. A request ID is a
client-generated correlation key, not an approval credential. A service must
reject reuse of an ID with a different decoded payload; retrying an already
committed identical request returns its recorded outcome instead of applying it
again. Rechecking revision remains mandatory when no recorded outcome exists.

This draft supports three operations:

| Operation | Transaction | Exact scope |
| --- | --- | --- |
| `replace_function_body` | Implementation or specification | Resolve a function ID and expected declaration hash, then replace only its brace-delimited body with `body`. Preserve name, parameters, return type, visibility, and effect bound. |
| `rename_symbol` | Implementation or specification | Resolve a function ID and expected declaration hash, then rename it to `new_name` and update all resolved references in the workspace snapshot. Preserve its ID, signature, effects, and behavior. |
| `replace_pact` | Specification only | Resolve a pact ID and expected declaration hash, then replace that single declaration with `declaration`. Preserve its name, target declaration, and pact ID; change only its clauses. |

`rename_symbol` initially accepts functions only. Its name must be a valid,
nonreserved language identifier. Reject collisions, ambiguous resolution, and
uneditable references. Do not rewrite comments, string contents, external
dependency sources, or unrelated matching spellings. Reject a public rename if
the trusted workspace configuration cannot enumerate and update its required
consumers; this operation is not a claim of compatibility for unknown clients.

Body and pact strings are parsed in the target's existing source context and must
consume exactly one body or pact, respectively. Trailing declarations, a changed
pact target, or a replacement containing only whitespace are rejected. These are
source strings, not structured ASTs or general-purpose file-write instructions.
Full-file replacement, adding or deleting declarations, moving declarations,
renaming types or pacts, changing signatures, and changing grants are outside
this protocol version. Unknown operations are rejected.

All target hashes are checked against the base snapshot. A request cannot target
the same identity twice. All operation footprints, including reference updates,
must be disjoint; reject an overlap rather than relying on array order. Operations
are proposed simultaneously against that base, then the whole candidate is
reparsed and checked. A no-op request is rejected as `PTN_CHANGE_INVALID_REQUEST`.

An implementation transaction cannot modify a protected pact, helper, fixture,
oracle, required evidence rule, accepted effect bound, or their resolved meaning.
Even a rename that edits a reference inside a protected pact needs a specification
transaction. The gate considers indirect changes, import rebinding, identity
loss, and dependencies; leaving the pact's text untouched is insufficient.

A specification transaction can propose protected changes but supplies no
approval. Before commit, a trusted external process must authorize the exact
base revision, candidate revision, policy baseline, and complete diff. There is
no `approved`, signer, or token field in a request. The approving authority comes
from trusted host/CI configuration and cannot be changed by the candidate.

## 6. Staging and atomic commit

The implementation order is normative even though the service is future work:

1. Decode and schema-check the request. Snapshot the workspace and external
   inputs; compare its revision and the external policy baseline. Resolve every
   target and compare its declaration hash. Reject stale or ambiguous targets.
2. Apply operations to an isolated candidate. Require edits and identity-ledger
   maintenance to stay within the declared operation footprints. Parse, resolve,
   type/effect-check, inventory obligations, and run the checks required by the
   external policy. A failure leaves the live workspace untouched.
3. Present the full source and identity diff, affected interfaces, changed or
   removed obligations, effect changes, performed checks, and remaining unknowns.
   These response schemas are deferred; a summarized preview cannot stand in for
   the exact bytes covered by approval. Dry runs stop here without modification.
4. For a specification transaction, obtain matching external approval. Approval
   for different bytes, a prior revision, or a different baseline is invalid.
5. At commit, atomically compare the current full input revision and policy
   baseline with those staged. Publish the complete candidate only if both still
   match. Otherwise return a conflict and publish none of it. Revalidate any
   external approval revocation according to the trusted approval service.

The exact file diff is an ordered inventory of each changed path with an old and
new content digest, plus explicit absence markers for addition or removal. Sort
by unsigned UTF-8 path bytes; bind all source, ledger, metadata, and protected
changes. The trusted approval record binds that inventory and both full revisions,
not human diff formatting. A review UI may render these bytes as ordinary diffs.

Atomicity includes every modified file and ledger entry. The eventual storage
implementation must provide crash recovery and a transaction mechanism that
prevents participating readers from observing partial publication. It must
exclude uncoordinated writers during the compare-and-publish operation, use a
storage mechanism that makes that operation atomic, or reject the commit in an
environment where it cannot provide those guarantees. A final file scan followed
by sequential overwrites is insufficient. Success describes the workspace at the
commit point; it does not prevent later external edits. No success may be reported
while recovery is still required. This milestone specifies the requirement, not a
filesystem coordinator or universal snapshot isolation for ordinary editors.

Resolve paths beneath the captured workspace root. Reject symlinks for editable
inputs or their path components, nonregular input files, and path aliases/case
collisions. Host file operations must prevent path or inode replacement between
validation and write, and reject multiply linked editable files. Hard links or
symlinks must not turn a workspace edit into a write outside the workspace.
Dependency sources are immutable input snapshots, not writable operation targets.

Direct text edits must pass the same external baseline gate in CI. An agent that
can bypass the local edit service still cannot redefine the trusted policy or
erase a protected obligation merely by bypassing this request format.

After publishing an approved specification edit, the trusted owner process must
record the approved protected contents as the new accepted policy baseline. The
request cannot install that baseline itself. If baseline advancement is pending,
report that state explicitly: publication is not deployment acceptance, and
implementation commits against the changed protected state remain blocked until
the trusted baseline covers it. Installing the new baseline changes the workspace
revision; subsequent clients must obtain both fresh revision and baseline values.
The adopted contents must match the exact approved diff; additional policy changes
need their own trusted approval.

## 7. Example payloads and expected validation

The files in [examples/protocol](examples/protocol) are small protocol fixtures,
not executable compiler tests. Repeated-character hashes and IDs stand for
fictional snapshots and declarations; they are not hashes of repository files.
For request examples, assume current workspace revision `sha256:` plus 64 `a`s,
policy baseline plus 64 `b`s, and target hashes exactly as shown unless noted.
Protected function ID `sym_` plus 32 `7`s is a contract helper. All other request
targets are ordinary functions except the explicitly identified pact.

| File | Schema result | Semantic expectation |
| --- | --- | --- |
| [diagnostic-overflow.valid.json](examples/protocol/diagnostic-overflow.valid.json) | Accept | Located checked-overflow trap; the fictional source must contain its declared span. |
| [diagnostic-counterexample.valid.json](examples/protocol/diagnostic-counterexample.valid.json) | Accept | Static countermodel with exact integer strings; not yet replayed. |
| [change-replace-body.valid.json](examples/protocol/change-replace-body.valid.json) | Accept | Stage the quota repair; require current hashes, successful checks, and policy acceptance. |
| [change-rename.valid.json](examples/protocol/change-rename.valid.json) | Accept | Stage a function rename if all consumers are editable and no protected references change. |
| [change-specification.valid.json](examples/protocol/change-specification.valid.json) | Accept | Stage the existing increment pact replacement; external approval is still required. |
| [diagnostic-i64-number.invalid.json](examples/protocol/diagnostic-i64-number.invalid.json) | Reject | An `I64` payload uses a JSON number instead of its exact decimal string. |
| [change-unknown-operation.invalid.json](examples/protocol/change-unknown-operation.invalid.json) | Reject | `write_file` is not an available operation. |
| [change-stale.invalid.json](examples/protocol/change-stale.invalid.json) | Accept | Reject semantically with `PTN_CHANGE_STALE_REVISION`: the base uses 64 `9`s rather than the current 64 `a`s. |
| [change-protected.invalid.json](examples/protocol/change-protected.invalid.json) | Accept | Reject semantically with `PTN_CHANGE_PROTECTED_TARGET`: an implementation transaction rewrites a protected helper. |

Additional semantic boundaries are `"9223372036854775808"` as an out-of-range I64,
`start_byte > end_byte`, an offset splitting a UTF-8 scalar, a reserved rename,
duplicate operation targets, and a valid source replacement with extra trailing
declarations. Each can pass structural checks and must still be rejected before
use or commit. Milestones 3, 10, 23, and 24 turn these specified outcomes into
executable conformance and transaction tests.
