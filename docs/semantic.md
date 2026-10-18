# Semantic workspaces and protected acceptance

The shared `semantic`, `workspace`, `acceptance`, and `editor` modules implement
the review and edit path. `cargo test -p pactern --test semantic_workspace`
exercises identity reconciliation, queries, concurrent transactions, strict JSON
decoding, durable publication, protected helpers, approvals, and editor positions.

## Identities and queries

`SemanticIndex::build` parses a closed `SourceSnapshot` with an `IdentityLedger`.
The ledger is authoritative and must be retained; the index is rebuildable and
can be discarded. IDs are allocated independently of source names and positions.
Deleted entries are retained and never reused. Exact module/kind/name matches
survive body edits. A unique token fingerprint, ignoring declaration name and
trivia, supports unchanged-body renames and module/file moves. Multiple possible
matches return `PTN_IDENTITY_AMBIGUOUS`; simultaneous arbitrary rewrites and moves
are not automatically reconciled. Semantic rename operations explicitly update
the ledger and therefore also preserve recursive function identities.

Source-only CLI queries without a persisted ledger use IDs provisional to their
source snapshot. Their deterministic snapshot namespace makes repeated identical
queries agree; persistence across edits requires a managed store or supplied
identity ledger. A provisional source query is not an editable managed revision.

`inspect`, `callers`, `impact`, and `affected_tests` return the exact revision and
a completeness marker. Impact follows reverse dependencies through callers,
interfaces, and pacts. Unresolved field/effect dependency information broadens
impact to the whole supplied package. Missing or incomplete test dependency
information selects the entire supplied test inventory. The caller must provide
the complete external test inventory; this API cannot discover tests outside it.
The current index resolves declaration navigation, not local-variable navigation.

The versioned [semantic request](spec/0.1.0/schemas/semantic-query-request.schema.json)
and [semantic response](spec/0.1.0/schemas/semantic-query-response.schema.json)
schemas cover inspection, callers, impact, and affected tests. `QueryRequest::decode`
rejects duplicate/unknown fields; `execute` rejects stale revisions. All responses
use `kind: "query_result"`, with an explicit completeness marker and reasons.
The [editor request](spec/0.1.0/schemas/editor-request.schema.json) and
[editor response](spec/0.1.0/schemas/editor-response.schema.json) schemas map to
`EditorRequest` and the same adapter APIs. `query_protocol` tests check request
decoding and emitted structures against these local schemas; the structural
smoke checker is not a general JSON Schema validator.

`editor::diagnostics` uses the same frontend as the CLI and translates source
ranges to zero-based UTF-16 positions, including non-BMP characters and CRLF.
`editor::definition` rejects a revision mismatch and returns a declaration range.
These are adapter APIs, not a separately implemented LSP server.

## Managed transactions

`WorkspaceData` contains all participating files, an authoritative ledger,
trusted dependency/configuration/tool inventory records, and request receipts.
`WorkspaceData::snapshot` uses the protocol's length-framed SHA-256 inventory,
including exact file bytes, ledger bytes, external records, and policy baseline.
The host must inventory real dependency content roots and pinned distributions;
empty external inventories only describe a dependency-free local profile and
must not be described as fully reproducible release evidence.

`ChangeRequest::decode` rejects duplicate JSON keys recursively, unknown fields,
unknown operations, invalid IDs/digests, trailing values, reserved rename names,
and empty operations. All target hashes are checked against the same base.
Footprints must be disjoint. The isolated candidate is reparsed and fully checked
before publication; body/pact replacements cannot inject declarations or change
signatures, pact targets, identities, or visibility. Failed or stale requests
publish no source or ledger changes. Request IDs reject different payload reuse
and return the recorded result for identical committed retries.

`Workspace` publishes an entire in-memory value under one mutex. `ManagedWorkspace`
persists that same complete object in `workspace.json`, holds a kernel file lock,
fsyncs a temporary complete snapshot, renames it atomically, and syncs the parent
directory. Readers use the same lock. A crash before rename leaves the old
snapshot plus an unreferenced temporary file; after rename the new snapshot
includes its retry receipt. Temporary files are never live source. A failed sync
does not report success; reopen and identical-request retry resolve uncertain
publication. The store rejects symlink components, nonregular files, and, on Unix,
multiply linked files.

The trusted host must exclusively own the managed directory and all participating
writers/readers must use this API. Advisory locks do not constrain arbitrary
external editors. Ordinary source-tree multi-file commits are therefore not
offered: export/import is explicit, and editing a checkout is not the same as
editing the managed snapshot. The implementation uses Unix-style durable atomic
rename; Windows durability/publication semantics have not been validated.

## External policy and approval

`TrustedBaseline::capture` is an owner operation, never a field in a change
request. Store the baseline outside candidate-controlled files. Protected files,
pacts, helper dependency closures, resolved identity bindings, and accepted
function signatures/effect bounds are checked against the fixed baseline.
A pact target implementation remains editable; helpers called by pact clauses
are protected. Incomplete protected dependency closure fails closed.
`TrustedBaseline::check` is the same check for semantic and direct-text candidates.

The baseline can require additional trusted checks. `stage_with` invokes a
host-selected checker and refuses publication if required check names are absent.
The default profile requires frontend checks; it does not claim proof, executed
tests, or release acceptance. Production hosts must wire the evidence gate and
required tests into the callback. Clients cannot provide check completion names.

Specification commits require a trusted `Approval` binding exact base revision,
candidate revision, policy baseline, and complete sorted file diff, including the
identity ledger. Missing or mismatched approval fails. The host revalidates
approval revocation and the current external baseline at commit. Approval creation
and deserialization are trusted host capabilities, not public authorization APIs.
After approved publication, the receipt reports `baseline_advancement_pending`;
ordinary implementation edits remain blocked until the external owner captures
and installs the accepted protected state. Publication does not deploy it.

## Fixed-tool repository CI gate

`crates/pactern/examples/acceptance_gate.rs` is a separate acceptance executable.
Compile it from the **externally selected baseline checkout**, then pass baseline
and candidate roots. It never loads candidate policy or invokes candidate scripts.
Its protected inventory includes specifications, fixtures, baselines, all crate
test sources, the baseline harness, CI workflows, scripts, and the gate itself.
It also protects manifests/lockfiles at every depth, Cargo configuration,
toolchain and test-runner configuration, build scripts, all pact files, and entire
Rust source files containing recognizable embedded test attributes or module/include
boundaries in either the baseline or candidate. Protecting module boundaries keeps
an unchanged test file from being silently disconnected. Detection is conservative:
matching comments/strings also protect a file; nonstandard test-generating macros
without recognizable Rust test/cfg attributes must be inventoried explicitly.
Added or removed protected files count as changes. The full inventory includes
untracked inputs, rejects path aliases and links, and binds the actual executing
gate binary. `.git`, `target`, `node_modules`, and `.DS_Store` are excluded outputs
or metadata. Inventories are bounded to 20,000 files and 128 MiB.

Run the baseline's wrapper, not a candidate copy:

```sh
sh "$TRUSTED_BASELINE/scripts/check-protected-baseline.sh" \
  "$TRUSTED_BASELINE" "$CANDIDATE"
```

The gate emits exact revisions, baseline, changed protected paths, and full diff.
An optional third argument to the executable is an approval JSON loaded by the
trusted owner from outside both checkouts. CI must obtain that file from protected
approval storage; moving a candidate file outside its checkout does not grant
authority. The default CI invocation supplies no approval. Initial installation
requires an externally reviewed baseline containing this gate: a missing gate
fails instead of compiling an untrusted candidate fallback. Branch protection
must require the baseline-defined workflow or an external check; a candidate's
workflow cannot grant itself required-check authority.

This repository gate protects byte inventories. Semantic project protection uses
`TrustedBaseline` and its resolved helper closure. Passing either protection gate
alone is not proof of program correctness. The release workflow additionally runs
the pinned full test/build/install checks.
