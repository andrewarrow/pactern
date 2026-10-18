# Scoped host capabilities

The static registry `quota+terminal/0.1.0` provides quota transactions and terminal
I/O. Both use opaque, scoped handles supplied by the embedding host. See
[terminal programs](terminal.md) for the terminal API and runnable example.

## Quota host

The quota adapter provides an opaque `host::quota::Txn` and four
operations. Root handles come from the trusted host. Source cannot construct,
copy, return, aggregate, compare, or instantiate generics with them.

| Operation | Signature | Effects |
| --- | --- | --- |
| `host::quota::read` | `(borrow Txn) -> I64` | `db.read` |
| `host::quota::reserve` | `(borrow Txn, I64) -> Bool` | `db.write` |
| `host::quota::commit` | `(Txn) -> Unit` | `db.write` |
| `host::quota::rollback` | `(Txn) -> Unit` | empty |

`reserve` stages a positive amount if quota suffices; insufficient quota returns
false. A nonpositive amount is a host error. `commit` and `rollback` consume the
token. Dropping a transaction never commits. Declared effects are upper bounds,
including for unreachable-looking calls and imports, and confer no authority.
All registry effect categories are recognized statically; quota and terminal
adapters are implemented. Missing host operations remain unresolved errors.

The ownership pass rejects use after a move on any incoming branch, outer-handle
consumption in a loop, mutable handles, and overlapping exclusive borrows during
left-to-right argument evaluation. It permits reborrowing a borrowed parameter.
Analysis is deliberately conservative: a move in an exiting branch may still
make a later use unavailable. Scoped exits and host revocation bound runtime
resources independently of the static checks.

`host::Invocation` owns grants, tokens, transactions, trace, and budgets. Grants
name a tenant and allowed read/write operations. An invocation has no grants
unless the embedding Rust host supplies them. Rust handle tokens have private
fields and are neither Copy nor Clone; tokens from another invocation are
rejected. Operations charge every active ancestor budget before proceeding.
Budgets cover operation count, serialized request bytes, cumulative host allocation
allowance, and elapsed deadlines. Wasm memory and instruction fuel are separately
bounded by the sandbox host. Child-created tokens expire when their scope ends.
Fatal budget/replay errors, cancellation, timeout, and scope unwinding revoke
authority and discard uncommitted changes. Ordinary operation errors, such as an
invalid reservation amount or a denied operation, can leave the invocation live;
an error escaping `scoped` revokes it. The compiled bridge revokes on every exit,
including failed host calls. An explicit commit already completed cannot be
undone by a later trap.

The initial `QuotaStore` is an in-memory transactional adapter, not durable SQL
storage. A transaction snapshots a tenant's balance and revision, stages changes,
and commits under a mutex only if that revision still matches. Competing commits
conflict and must retry. Balances cannot be negative; tenant grants are checked
before host access. The concurrency test starts 16 competing workers against 100
units, observes exactly 100 successful reservations, and verifies another tenant
is unchanged. A production durable adapter remains separate work.

Recording stores exact ordered requests and results, including failures, with a
source revision and policy hash. Replay checks both bindings, exact requests,
response shapes, missing/redacted data, and complete consumption. It reconstructs
token liveness while substituting recorded results and never calls the live store.
Tests replay successful writes and failed commits and assert no live write count
increase. Recorded failures can be replayed only against their original revision.
Traces may contain tenant names and data; the embedding service controls retention.

Run the host tests and the integrated quota workflow:

```sh
cargo test -p pactern --test effects --test host --test backend_quota --test backend_quota_review --locked
cargo run -p pactern --example quota_workflow --locked -- target/quota-workflow
```

The workflow invokes a pure guarded Wasm quota rule inside a Rust-owned
transaction. The separate [compiled quota example](../examples/quota/src/transaction.ptn)
uses borrowed reads/reservations and owning commit/rollback through the opt-in
Wasm import bridge. The embedding API supplies root handles; the default host and
CLI do not grant quota access. Generated code releases remaining owned handles at
source-scope exits, while borrowed parameters retain their caller's owner.
Implicit release discards pending state without consuming the operation budget
or adding external trace events. Traps and fuel-sliced cancellation/deadline
checks revoke the entire invocation. See the [exact import ABI](backend.md).
