# Pactern 0.1.0 conformance corpus

This corpus records expected behavior from the Pactern language 0.1.0 specification and its normative examples. The syntax-only library parses every source file here and checks the recorded syntax and parse outcomes. Runtime, pact execution, and verification expectations remain fixture contracts for future implementations; no Pactern program has been executed.

`manifest.json` is the machine-readable index. Each case has a stable ID, a source path, an expected outcome, and, when relevant, an entry point, arguments, protected contracts, or pact phase statuses. `I64` arguments and results use canonical decimal JSON strings. Strings remain JSON strings, and lists and records use JSON arrays and objects. `Result` values use a `tag` plus their payload or error name. Static diagnostics use stable `PTN_*` identifiers; where the protocol registry does not yet define a language diagnostic, this corpus assigns a descriptive code for the specified condition.

The sources cover checked integer boundaries and evaluation order, lists, strings, mutation, matching, records, quota success and recoverable errors, preconditions and postconditions, contract evaluation traps, and the initial verifier's supported and unsupported boundary. The quota variants preserve the `quota` module name so the same protected pact can be applied to each candidate. They intentionally violate the protected postcondition: one returns unchecked arithmetic and one rejects every request.

The current manifest contains 79 expected cases. The `pacts` values describe runtime guard outcomes only; verification results appear separately as `verification` outcomes. These expected outcomes are transcribed from the spec and are not execution results.

| Case group | Governing rules |
| --- | --- |
| Integer limits, operators, ordering, strings, and short-circuiting | [Language §4](../../docs/spec/0.1.0/language.md#4-integers-and-evaluation), [normative integer examples](../../docs/spec/0.1.0/examples.md#checked-integers-and-evaluation-order) |
| Lists, records, local mutation, inference, `Option` patterns | [Language §3](../../docs/spec/0.1.0/language.md#3-values-and-types) and [§5](../../docs/spec/0.1.0/language.md#5-control-flow-and-errors) |
| Shadowing and public signatures | [Language §2](../../docs/spec/0.1.0/language.md#2-source-names-and-modules) |
| Quota results, faulty candidates, and pact phase order | [Quota example](../../docs/spec/0.1.0/examples.md#recoverable-errors-and-a-protected-pact), [language §8](../../docs/spec/0.1.0/language.md#8-pacts-and-runtime-contracts) |
| Contract scope, clause types, evaluation traps, and short-circuit guards | [Contract edge cases](../../docs/spec/0.1.0/examples.md#contract-edge-cases-and-proof-boundaries), [language §8](../../docs/spec/0.1.0/language.md#8-pacts-and-runtime-contracts) |
| Proved/refuted consistency or postconditions and unsupported loops | [Language §9](../../docs/spec/0.1.0/language.md#9-initial-verification-boundary) |

Each case selects exactly one implementation file plus its listed contract
files. A future compiler adapter must stage these files using their declared
module paths, rather than compile the whole corpus as one package. In particular,
quota variants replace `src/quota.ptn`; they are never loaded beside it. Static
diagnostic cases expect the specified code and phase without pinning message
wording. Syntax errors include local UTF-8 byte and line/scalar-column spans;
later semantic and protocol diagnostics are not implemented yet.

`args` is an array of values, not source expressions: `List<I64>` uses arrays of
decimal strings, string arguments contain their decoded text, and `Option`
uses `{"tag":"Some","value":"9"}` or `{"tag":"None"}`. An entry point's
signature determines the type. `return` outcomes contain a value, `trap` outcomes
contain a category and (for contract evaluation) underlying cause, `diagnostic`
outcomes contain a phase/code, and `verification` outcomes contain a distinct
claim/status. Missing pact clause phases impose `true` as specified. A phase
that cannot run is `not_reached`.

Run `cargo test --workspace --locked` to parse all corpus sources, validate the
recorded syntax outcomes, and run the Rust seed comparison. These checks do not
execute the Pactern programs or verify their runtime and proof expectations. See
the [baseline guide](../../baselines/README.md) to run both comparison languages
and record costs. The suite is a seed, not exhaustive coverage: module resolution,
generic/effect/handle checks, host limits, solver timeouts, and other cases will
grow with their implementation milestones.
