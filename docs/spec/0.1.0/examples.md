# Core examples and expected outcomes

These are normative examples for [language version 0.1.0](language.md), not output
from an existing compiler. Module fragments are illustrative source; tables give
the expected outcomes for conformance fixtures. The milestone 3
[seed corpus](../../../fixtures/conformance/README.md) records machine-readable
expectations; its Rust harness validates metadata and runs Go/Rust comparisons,
without executing Pactern.

## Checked integers and evaluation order

| Expression or source | Expected outcome |
| --- | --- |
| `9223372036854775807` | Valid `I64` maximum |
| `-9223372036854775808` | Valid `I64` minimum |
| `9223372036854775808` | Static integer-literal range error |
| `-(9223372036854775808)` | Static range error for the inner positive literal |
| `01` | Invalid integer token; not decimal one |
| `9223372036854775807 + 1` | Well-typed; integer-overflow trap if evaluated |
| `-(-9223372036854775808)` | Integer-overflow trap |
| `-7 / 3` | `-2` |
| `-7 % 3` | `-1` |
| `1 / 0` or `1 % 0` | Division-by-zero trap |
| `-9223372036854775808 / -1` | Integer-overflow trap |
| `-9223372036854775808 % -1` | Integer-overflow trap, even though the mathematical remainder is zero |
| `false && (1 / 0 == 0)` | `false`; the right operand is not evaluated |
| `true || (1 / 0 == 0)` | `true`; the right operand is not evaluated |
| `1 < 2 < 3` | Syntax error; comparisons cannot be chained |

```text
module order;

fn first(a: I64, b: I64) -> I64 {
    return a;
}

pub fn run() -> I64 {
    return first(1 / 0, 9223372036854775807 + 1);
}
```

`run` traps on division by zero. It never evaluates the second argument or enters
`first`. The backend may not reorder the overflow ahead of the division.

## Values, mutation, and matching

| Source | Expected outcome |
| --- | --- |
| `let count = 1; count = 2;` | Static assignment-to-immutable-binding error |
| `let mut count = 1; count = 2;` | Valid; the binding becomes `2` |
| `let mut count = 1; count = true;` | Type error; reassignment cannot change its type |
| `let values: List<I64> = [];` | Valid empty list |
| `let values = [];` | Cannot infer the element type |
| `[10, 20][-1]` or `[10, 20][2]` | Bounds trap |
| `"é"[0]` | Type error; string indexing is unavailable |
| `"a" == "\u{61}"` | `true` |
| `"\u{D800}"` | Invalid Unicode scalar escape |
| `let values = [1, 2]; values[0] = 3;` | Rejected; assignment targets are local binding names only |
| `let x = 1; if (true) { let x = 2; }` | Rejected; an active local binding cannot be shadowed |

```text
module option_example;

pub fn unwrap_or_zero(value: Option<I64>) -> I64 {
    return match (value) {
        Option::Some(number) => number,
        Option::None => 0,
    };
}
```

Removing the `None` arm makes the match non-exhaustive. Putting `_ => 0` first
makes later arms unreachable and is rejected. `Option::Some(_)` alone does not
cover `None`. Matching an `I64` by a finite list of literals needs a final wildcard
or binding pattern.

## Recoverable errors and a protected pact

Implementation file `src/quota.ptn`:

```text
module quota;

pub enum ReserveError {
    InvalidAmount,
    InsufficientQuota,
}

pub fn reserve(available: I64, amount: I64) -> Result<I64, ReserveError> {
    if (amount <= 0) {
        return Result::Err(ReserveError::InvalidAmount);
    }
    if (amount > available) {
        return Result::Err(ReserveError::InsufficientQuota);
    }
    return Result::Ok(available - amount);
}
```

Protected file `contracts/quota.pact.ptn`:

```text
module quota;

pact reserve_safety for reserve {
    requires available >= 0;
    ensures match (result) {
        Result::Ok(remaining) =>
            amount > 0 && amount <= available &&
            remaining == available - amount && remaining >= 0,
        Result::Err(ReserveError::InvalidAmount) => amount <= 0,
        Result::Err(ReserveError::InsufficientQuota) =>
            amount > 0 && amount > available,
    };
}
```

| Invocation | Expected result |
| --- | --- |
| `reserve(3, 2)` | `Result::Ok(1)` |
| `reserve(3, 4)` | `Result::Err(ReserveError::InsufficientQuota)` |
| `reserve(3, 0)` | `Result::Err(ReserveError::InvalidAmount)` |
| `reserve(0, 1)` | `Result::Err(ReserveError::InsufficientQuota)` |
| `reserve(9223372036854775807, 1)` | `Result::Ok(9223372036854775806)` |
| `reserve(-1, 1)` | Precondition trap before the implementation executes |

`Err` returns normally and is checked by the postcondition. Replacing the body
with `return Result::Ok(available - amount);` yields a valid typed candidate but
violates the pact for `(3, 4)`. A static check should refute the postcondition with
that or another concrete counterexample. With guards, the faulty call traps
instead of returning a successful negative quota.

Always returning `InvalidAmount` also violates the pact for `(3, 2)`. The pact
therefore requires successful handling of valid reservations, rather than merely
allowing an implementation to reject every request.

## Contract edge cases and proof boundaries

These clauses are considered separately on an otherwise suitable target.

| Contract or implementation | Expected outcome |
| --- | --- |
| `requires amount > 0; requires amount < 0;` | Inconsistent assumptions: reject an accepted proof; runtime calls fail a precondition |
| `requires result == 0;` | Scope error: `result` exists only in postconditions |
| `requires 1;` | Type error: a clause must be `Bool` |
| `requires 1 / divisor > 0;` with divisor zero | Contract-evaluation trap with division-by-zero cause |
| `requires divisor != 0 && 1 / divisor > 0;` with divisor zero | False precondition, without evaluating division |
| An effectful helper called by a pact | Static contract-purity error |
| A loop or recursive function submitted to the first prover | `unsupported`; it may still type-check, run, and have runtime guards |
| A solver times out | `unknown`, never `proved` |
| A bounded search finds no failing input | Pass only within the recorded bound, never an unbounded proof |
| A body traps before returning | Invocation failure; no successful postcondition check or passing acceptance test |
| A postcondition is proved for a supported function | Proof of that stated claim under recorded assumptions; no guarantee against host resource exhaustion |

A direct edit that weakens `reserve_safety`, deletes its `requires` clause, changes
an expected-value fixture, or weakens a helper used by the pact is a specification
change. It is rejected through the implementation-edit path even if the source
remains well-typed and all remaining tests pass. Legitimate requirement changes
use the separate policy-governed specification path.

## Effects and handle ownership

This example assumes a future registered host adapter with these signatures:

```text
host::quota::read(txn: borrow host::quota::Txn)
    -> Result<I64, host::quota::StoreError> effects { db.read }
host::quota::write(txn: borrow host::quota::Txn, remaining: I64)
    -> Result<Unit, host::quota::StoreError> effects { db.write }
```

The signatures above describe registry entries; they are not user-defined
`extern` declarations or executable adapter code. The host supplies an owning
transaction handle to an entry point. Ordinary helper source can borrow it:

```text
module reader;
import host::quota;

pub fn read_quota(txn: borrow host::quota::Txn)
    -> Result<I64, host::quota::StoreError> effects { db.read } {
    return host::quota::read(borrow txn);
}
```

| Attempt | Expected outcome |
| --- | --- |
| Omit `effects { db.read }` from `read_quota` | Static effect-bound violation |
| Declare `db.read` but invoke the registered write operation | Static effect-bound violation |
| Declare `db.write` but hold only a read-authorized handle | Host denies the write before acting; the declaration grants no permission |
| Use a tenant-A handle to act on tenant B | Host rejects the resource scope before any action |
| `let next = txn;` then use owning `txn` again | Static use-after-move error |
| Pass `borrow txn` twice in the same call | Static overlapping-borrow error |
| Evaluate `f(borrow txn, read(borrow txn))` | Static overlapping-borrow error; the first argument's borrow is still active |
| Move a borrowed parameter into an owning parameter | Static ownership error |
| Return a handle or place it in `Result::Ok` | Static unsupported-handle-placement error |
| Construct `host::quota::Txn { ... }` in source | Static opaque-type construction error |
| Exhaust guest fuel while a transaction is open | Invocation traps; host rolls back and revokes handles even if guest cleanup cannot run |

For the eventual quota service, the host reads and updates stored quota within
one transaction; the caller supplies the requested amount, not its own claimed
available balance. Commit requires the adapter's explicit success protocol.
Releasing a handle cannot commit. Atomicity, retry behavior, and authorization
must be tested independently from the pure `reserve` proof.
