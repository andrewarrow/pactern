# Pactern core language 0.1.0

Status: draft specification, revision 1. This defines intended behavior; no
compiler is implemented. The [grammar](grammar.ebnf) defines syntax, this document
defines its meaning, and [examples](examples.md) resolve representative edge cases.
The version applies to the whole [specification bundle](README.md).

## 1. Scope and conformance

A conforming core implementation must preserve the observable values, errors,
traps, effect order, and resource authority defined here. Implementation language,
garbage-collection strategy, and optimization do not change these rules. The
reference interpreter and C-to-WebAssembly backend will implement the same core.

| Included in the initial language | Deferred or rejected in this version |
| --- | --- |
| `Bool`, checked `I64`, `String`, `Unit`, lists, records, tagged unions | Floating point, wrapping arithmetic, raw pointers, null |
| Named functions, explicit signatures, local inference, simple generics | Closures, first-class functions, methods, traits, inheritance |
| Modules, `if`, `while`, exhaustive `match`, explicit local mutation | Async, task creation, shared mutable objects, exceptions |
| Pure contract expressions and declared effect bounds | User-defined effect handlers, arbitrary FFI, reflection, macros |
| Host-issued, scoped resource handles | Handle constructors, handle returns, handles inside aggregates |

The language subset and proof subset are different. Loops and ordinary recursion
can execute, but the initial static verifier supports only the fragment in §9.
An implementation must reject unavailable language features explicitly; it must
not silently reinterpret them or claim verification support for them.

## 2. Source, names, and modules

Source is UTF-8 without a byte-order mark. Invalid UTF-8 is a source error. LF,
CRLF, and lone CR each count as one newline; offsets refer to the actual bytes,
without normalization. Whitespace is ASCII space, tab, CR, or LF. Whitespace and
comments separate tokens; newlines do not terminate statements.
Comments are `//` to end of line or non-nesting `/* ... */`. Unterminated comments
and strings are errors. Identifiers use ASCII letters or `_`, followed by ASCII
letters, digits, or `_`; the bare `_` is a pattern wildcard. Names are case-sensitive.
Grammar keywords are reserved, including `result`.

Every file starts with `module path;`. A package has one implementation file
`src/<path>.ptn` per module, replacing `::` with directory separators. Its optional
contract file is `contracts/<path>.pact.ptn`, declaring the same module. An
implementation file contains functions and type declarations; a contract file
contains pacts. Both may import modules. Contract-only helpers are ordinary pure
functions whose definitions and dependencies are protected by acceptance policy.

`import util::math;` permits qualified references such as `util::math::add`.
Imports have no initialization effects. Package/dependency identities are resolved
from locked metadata; source names alone never select an arbitrary network fetch.
This version does not define a registry or executable dependency build scripts.

Own-module declarations use unqualified names. Imported declarations must be
qualified and `pub`; a public function cannot expose a private user-defined type.
Declarations share one module namespace and duplicates are errors. Struct fields
and enum variants have their own namespaces. All fields/variants are accessible
where their containing type is accessible. Local bindings cannot shadow a parameter
or an active local binding, including an outer-block binding. A name may be reused
after its earlier scope ends. Forward references within a module are allowed.

The prelude supplies `Bool`, `I64`, `String`, `Unit`, `List<T>`, `Option<T>`, and
`Result<T, E>`. These names cannot be redeclared as types or generic parameters.
`std` and `host` module roots belong to the toolchain. Host declarations come from
a versioned registry with types, effect bounds, handle modes, and failure behavior;
user source cannot declare an `extern` function or a new host capability.

Module import cycles are rejected in this initial version. Nongeneric functions
within a module may call each other recursively. Generic recursion, including
mutually recursive generic instantiations, is unsupported and rejected.

## 3. Values and types

Types are static and nominal: two separately declared records or enums remain
different types even when their fields or variants match. There are no implicit
numeric conversions, truthiness conversions, structural casts, or subtyping.

| Type | Values and operations |
| --- | --- |
| `Bool` | `true`, `false`; logical operations and equality |
| `I64` | Signed integers from `-9223372036854775808` through `9223372036854775807` |
| `String` | Immutable valid UTF-8; byte-exact equality, without Unicode normalization |
| `Unit` | The single value `()` |
| `List<T>` | Immutable finite ordered values of one ordinary value type |
| Record | All named fields present exactly once; immutable after construction |
| Tagged union | Exactly one declared variant with its declared payload types |

`Option<T>` has `Option::Some(T)` and `Option::None`. `Result<T, E>` has
`Result::Ok(T)` and `Result::Err(E)`. Constructors are qualified by the enum name;
unit variants omit parentheses. Record construction names fields and may list them
in any order. Field expressions evaluate in written order. Missing, extra, or
duplicate fields are errors.

An ordinary value can be assigned or passed without consuming its original
binding. This is value semantics; an implementation may share immutable storage.
Object addresses, identity, collector timing, and allocation placement are not
observable. There are no finalizers. Resource exhaustion remains observable as a
trap, and allocation costs must be measured rather than assumed away.

Parameters always have declared types and functions always declare a return type.
Parameter bindings are immutable. `let name = expression;` infers a local type;
an annotation is required when inference is ambiguous. `let mut` permits later
replacement of that binding's entire value, with the same type. Field assignment,
list-element assignment, mutation through aliases, and uninitialized bindings are
not supported. Assigning to an immutable binding is a static error.

Generics are rank-1 type parameters on named functions, records, or enums. No
overloading or implicit trait search occurs. A generic body must type-check for
every allowed ordinary value type; it cannot perform integer addition on an
unconstrained `T`. Calls infer type arguments from arguments and expected result
type, or supply them explicitly as `identity::<I64>(1)`. Empty lists, uninhabited
variant choices, and unconstrained generic calls require enough type context.
Handles are excluded from generic instantiation.

A call's callee must name a declared function or an enum constructor. Function
names cannot be stored as values, and a field or computed expression cannot be
called. Permissive postfix grammar does not enable methods or first-class calls.

`==` and `!=` are defined only for equal operand types among `Bool`, `I64`,
`String`, and `Unit`. Ordering is defined only for `I64`. Aggregate equality and
operator overloading are deferred. List indexing takes `I64`; a negative index or
an index at least the length traps. String indexing is a type error. All container
lengths fit in nonnegative `I64`; an impossible allocation or length traps before
creating a value.

## 4. Integers and evaluation

Integer tokens are decimal digits with no leading zero except `0`; there are no
suffixes or separators. A positive literal must fit in `I64`. The sole exceptional
magnitude `9223372036854775808` is allowed only as the direct operand token of
unary `-`, producing `I64` minimum. Trivia between the sign and token is permitted.
Thus `-9223372036854775808` is valid, but `-(9223372036854775808)` has an invalid
inner positive literal. Negative integer patterns follow the same range rule.

Arithmetic is checked in every build mode:

- `+`, `-`, `*`, and unary `-` trap when the mathematical result is outside `I64`.
- Division truncates toward zero. Remainder has the dividend's sign or is zero.
- `/` and `%` trap on a zero divisor. Both also trap for `I64` minimum and `-1`;
  generated C must check this case before performing either operation.
- There is no wrapping, saturation, implementation-defined signed overflow, or
  change in behavior between interpreter, debug, and optimized artifacts.

Operands, function arguments, list elements, and record fields evaluate left to
right. The first trap aborts the invocation; later operands and effects do not run.
`&&` and `||` short-circuit and require `Bool`. Unary `!` requires `Bool`; unary
`-` requires `I64`. Parentheses group expressions. Comparison and equality operators
are non-associative: use `a < b && b < c`, not `a < b < c`.

A constant operation such as `9223372036854775807 + 1` is well-typed but traps if
executed. Constant folding must preserve whether and when that trap occurs.
An out-of-range literal is instead a static error. Verification may refute an
obligation involving a well-typed trapping operation before deployment.

Strings use double quotes and allow `\"`, `\\`, `\n`, `\r`, `\t`, and
`\u{H...}` with one through six hexadecimal digits denoting a Unicode scalar
(`0` through `10FFFF`, excluding `D800` through `DFFF`). Raw control characters,
raw line breaks, unknown escapes, and invalid scalars are rejected. Here raw control
characters mean U+0000 through U+001F, as in the grammar. Source spelling
does not change the decoded value; `"a"` equals `"\u{61}"`.

## 5. Control flow and errors

Statements ending in `;` include bindings, assignments, expression statements,
`return`, `break`, and `continue`. Blocks, `if`, and `while` do not take a trailing
semicolon. Conditions use parentheses and must be `Bool`. Only a `Unit` expression
can stand alone as an expression statement. There are no implicit tail returns.
A non-`Unit` function must return a correctly typed expression on every reachable
normal exit. A `Unit` function may fall through or use `return;`.

`while` reevaluates its condition before each iteration. `break` exits the
innermost loop; `continue` begins its next condition evaluation. Both are invalid
outside a loop. Implementations may conservatively require a final return when
they cannot establish that a loop never exits. This version guarantees no general
termination analysis; execution remains bounded by the host.

`match (value) { pattern => expression, ... }` evaluates its input once and returns
one arm expression. All arms have the same type. Patterns include literals,
bindings, `_`, and qualified enum variants with nested payload patterns. Bindings
exist only within their arm. Matching proceeds in written order. Matches must be
exhaustive; unreachable arms are rejected. There are no guards, or-patterns, or
record/list destructuring. Infinite domains and types without destructuring
require a catch-all arm. Match exhaustiveness covers nested enum payloads too.

`Result::Err` is an ordinary return value, not a thrown exception. Callers inspect
it through `match`. The type communicates possible failure; application contracts
and tests establish appropriate handling. There is no `try`, `catch`, `throw`, or
`?` propagation operator in this first core.

Traps are separate from ordinary errors and cannot be caught by Pactern source.
They end the invocation and are reported by the host with their category, origin,
and available source information. Categories include arithmetic overflow, division
by zero, bounds, contract failure, capability denial, and resource exhaustion.
Database conflicts or transport failures may return a declared host `Result`;
unauthorized access must fail before the external action and cannot become success.

## 6. Effects and the host boundary

A function declares an upper bound with `effects { db.read, db.write }` between
its return type and body. The set has no duplicates; order has no meaning. An
omitted clause is the empty set. The rule applies to private and public functions:
inference determines what the body uses but does not expand the declaration.

For each call, the callee's declared effects must be contained in the caller's
declared set. Check all calls, including calls in unreachable-looking branches;
optimization cannot erase a required declaration. Imports do not erase effects.
The static effect registry starts with `db.read`, `db.write`, `fs.read`, `fs.write`,
`net.request`, `clock.read`, `random.read`, `env.read`, and `ai.infer`. These names
identify categories, not a promise that every adapter ships in the first runtime.
Unknown effect labels and missing host adapters are errors. General process
execution is outside the initial core.

Local computation, allocation, local mutation, and traps do not require an external
effect. In particular, “pure” does not imply termination or unlimited memory.
Contracts cannot call functions with external effects or inspect resource handles.

An effect declaration grants no authority. Every external operation uses an opaque
host-issued handle, and the host validates its invocation, permitted operation,
resource/tenant scope, liveness, and remaining budget before acting. Guest source
cannot forge handles, widen their scope, or grant itself a resource by changing a
manifest. Host policy and acceptance configuration are supplied outside candidate
changes. Imports and dependencies receive no ambient authority.

An artifact's imports must match the versioned host registry. The first host denies
all resources unless expressly granted. Budgets cover guest execution and memory
as well as nested operation counts, bytes, deadlines, and applicable spending.
Nested calls charge their active ancestor budgets; an exhausted budget prevents
the operation. Host-side timeouts and size limits apply even when a guest is not
executing. Language-level effects do not by themselves enforce these checks.

## 7. Handle ownership

To keep the first model implementable, handles have deliberately limited use.
The host issues root handles as entry-function parameters. Source cannot create
a handle, return one, put one inside a record/list/enum/`Option`/`Result`, capture
one, compare one, inspect its bits, or instantiate a generic type/function with
one. Host operations in this version also cannot return handles. A storage host
can supply a transaction handle scoped to an entire invocation.

An owning parameter `txn: host::quota::Txn` receives ownership. `let next = txn;`
or passing `txn` to another owning parameter moves it; the old binding is unusable.
The checker rejects a use unless ownership remains available on every incoming
control-flow path. A loop cannot consume an outer handle on one iteration and
reuse it on a later iteration. Handle bindings cannot be `mut`.

A parameter `txn: borrow host::quota::Txn` temporarily borrows an existing handle.
Its caller writes `borrow txn`. Borrow arguments must be handle binding names,
not arbitrary expressions, and the `borrow` parameter modifier applies only to
registered handle types. A borrow begins when its argument is evaluated, remains
active while later arguments evaluate, and ends when the synchronous call returns.
Thus `f(borrow txn, read(borrow txn))` overlaps borrows and is rejected, while
`f(read(borrow txn), borrow txn)` can be valid for a suitable signature. Source
cannot store or return them or move a borrowed handle into an owning parameter.
A borrowed parameter can be reborrowed by a nested call. The initial rule permits
only one active borrow of a handle: passing the same handle twice in one call,
or mixing a move and a borrow in that call, is rejected. A borrow is exclusive
access to the handle, but still grants only its existing resource permissions.

Normal scope exit releases still-owned handles in reverse acquisition order,
including early return, `break`, and `continue`. A moved handle is released by its
new owner. Cleanup is not an opportunity for user-defined effectful finalizers.
The host tracks all handles independently and cleans up or revokes them on traps,
timeouts, cancellation, or guest failure; guest cleanup may be impossible then.
Releasing a transaction handle never implicitly commits. Transaction commit and
rollback follow an explicit host adapter protocol, and aborted invocations cannot
leave a partial transaction. Storage atomicity is an adapter obligation, not a
consequence of the handle's type.

## 8. Pacts and runtime contracts

Pacts are named declarations in a module's contract file:

```text
module numbers;

pact increment_safety for increment {
    requires value < 9223372036854775807;
    ensures result == value + 1;
}
```

The target must be a nongeneric function in the same module. At least one
`requires` or `ensures` clause is mandatory. Every clause has type `Bool`.
Multiple clauses, and multiple pacts on one target, are conjunctions. They evaluate
within each phase in contract-file order and clause order within a pact: all
`requires` at entry, all `ensures` on normal return. Each phase stops at its first
false clause or trap. A missing clause kind imposes `true` for that phase.
Contract target names and
parameter names must resolve; an orphan pact is an error.

Clauses can reference the original ordinary value parameters, literals, pure
operators, and protected pure helper functions. `result` is a reserved expression
available only in `ensures`, with the target's return type. Parameters are immutable,
so postconditions refer to their original values without an `old` operator.
Handles are inaccessible in clauses. Helpers and the transitive definitions,
imports, constants, fixtures, and oracles that determine accepted meaning belong
to the protected specification, separately from the implementation under test.

Arguments are evaluated before entering a function. All preconditions are then
checked before its body executes; postconditions run after a normal result is
computed but before returning it. A false clause traps. A trap while evaluating a
clause is a contract-evaluation failure with the underlying cause preserved; it
does not count as a false premise or a successful check. Preconditions on an
external entry are checked too. A caller may not merely assume they hold.

The initial execution model retains these checks in all build modes even when
static proof succeeds. Guard elimination is deferred. A postcondition is not
evaluated after the body traps or fails to terminate. Failed execution is never
an acceptance-test success, and guards cannot undo external effects already
performed. Persistent invariants require appropriate host transactions and
storage constraints in addition to contracts.

Implementation changes cannot modify accepted pacts, their meaning-defining
dependencies, grants, or acceptance criteria. Such changes require a distinct
specification transaction evaluated against trusted external policy. Deleting a
pact, strengthening a precondition, or weakening a postcondition is a specification
change even if the candidate retains as many passing proofs. Approval binds to
the exact baseline and proposed diff, not to an agent-supplied Boolean.

## 9. Initial verification boundary

The first prover supports loop-free pure functions with acyclic calls, `Bool`,
checked `I64`, and finite nonrecursive records/enums including `Option`/`Result`
whose fields lie in the supported fragment. It may inline supported callees and
protected helpers. It does not assume arbitrary callee postconditions as axioms.
Strings, lists, recursive data, generic targets, effects, loops, recursion, host
functions, and heap reasoning return `unsupported` for static proof while remaining
subject to applicable typing, testing, and runtime guards.

Verification uses exact machine-integer semantics with explicit trap obligations.
It records separate claims for precondition consistency, postcondition validity,
absence of language traps, and termination. Inconsistent preconditions are
rejected; an unsatisfiable premise cannot earn an accepted vacuous proof.
The straight-line fragment's termination claim assumes sufficient execution
resources. Absence of language traps does not prove that host fuel, memory,
deadlines, or external services can never fail; those assumptions must be visible.

Each static claim is `proved`, `refuted`, `unknown`, or `unsupported`. Timeouts are
`unknown`. Bounded exploration records its bound and never counts as unbounded
proof. Runtime guard status and test results are separate dimensions. The later
artifact-linked evidence format must record the exact checked inputs and trusted
toolchain assumptions. Diagnostics in [protocol.md](protocol.md) describe a result;
they are not themselves an acceptance certificate.

Policy rejects refuted obligations and failed required tests. `unknown`,
`unsupported`, missing evidence, and skipped tests cannot satisfy a proof
requirement. An unproved, unguarded guarantee must never be assumed downstream.
Owner-defined specifications can still be wrong or incomplete; language checks
establish only the stated properties under their stated assumptions.
