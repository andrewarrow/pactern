# Pure guest backend (ABI 1)

`backend::emit_c` lowers checked core to freestanding C11. `emit_c_guarded`
also inserts the attached preconditions and postconditions at every call.
`compile_wasm` invokes Clang and wasm-ld without libc or WASI; `run_wasm` runs the
result with Wasmi 2.0.0. All imports are rejected, including unused imports.
This is a core Wasm module, **not a Component Model component**.

The supported surface includes immutable strings, lists, records, enums,
Option/Result, generic helpers through type erasure, nested matching, structural
equality, short-circuit expressions, loops, and checked I64 arithmetic. Explicit
statement ordering preserves left-to-right operand/argument evaluation. Entry
points must be nongeneric; arguments are validated and embedded into an invocation
artifact. This is not yet a reusable dynamically parameterized guest entry ABI.
The default emitter rejects host calls and nonempty effect bounds. The explicit
quota embedding API below supports only registered quota effects.

## Build tools

Set `PACTERN_CLANG` and `PACTERN_WASM_LD` to override tool discovery. The candidate
CI uses Clang 21 and wasm-ld 21 on macOS. Homebrew LLVM/lld 21 and unversioned installations are
discovered on macOS; PATH is the fallback. The compiler and linker run separately,
so no target sysroot is needed. `PACTERN_NATIVE_CLANG` selects the compiler for the
native sanitizer harness; its default is `clang` so macOS uses the host SDK's
compatible sanitizer runtime.

```sh
cargo test -p pactern --locked --test backend
cargo test -p pactern --locked --test pacts
```

Missing compiler tools fail backend tests. Some solver-dependent pact tests skip
when Z3 is unavailable; the full candidate gate also runs an installed `verify`
example, which must produce actual proofs and fails without the solver. CI checks
the external tool versions before running the gate. The native sanitizer
executable is killed after 15 seconds if it hangs.

## Values, roots, collection, and traps

`runtime/pactern.c` is the exact runtime embedded in generated source. Values are
16-byte `{i64 payload, u32 tag, u32 heap_offset}` records. Tags 0 through 6 mean
Unit, Bool, I64, String, List, Record, and Variant. Object headers contain size,
allocation/mark flags, kind, length, and variant identity. Heap offsets are local
to one invocation and never authority-bearing host handles.

The nonmoving first-fit heap defaults to 4 MiB. Allocation retries after a precise
mark/sweep collection; sweeping coalesces adjacent free blocks. Every compiler
variable and expression temporary occupies an explicit root slot. Frames link
into a root chain. Scalar bits are never scanned for pointers. Arrays and records
are immutable after construction, so copying values can safely copy their
references. Collection traces exactly their child slots. Function exits unlink
frames; completed and trapped invocations discard all roots and collect the heap.
The native ASan/UBSan harness tests allocation, transitive roots, ignored scalar
pointer bits, reclamation, overflow, bounds, zero division, and cleanup.

Compiler frames are limited to 4096 slots; execution permits 32768 simultaneously
active slots, call depth at most 64, and value nesting at most 64. This bounds root
storage below the linked 1 MiB C stack with room for runtime frames. Language fuel
charges statements, expressions, function entries, value nodes and string bytes,
as in the reference interpreter. The Wasm host adds independent instruction fuel,
maximum linear memory (default 16 MiB), input module bytes (8 MiB), and decoded
output bytes (1 MiB). Wasm initialization is metered too. Exact acceptance at a
memory or Wasm-instruction limit is backend-specific.

Trap codes are 1 overflow, 2 division by zero, 3 bounds, 4 execution limit,
5 invalid invocation, 6 compiler invariant, 7 precondition, 8 postcondition,
9 contract evaluation. A contract evaluation trap exports one cause code.
Nested guard failures can have deeper cause chains in the reference interpreter;
the current guest ABI exposes only the outer cause code. Guest traps do not yet
carry source spans. Arithmetic has no signed-overflow undefined behavior;
multiplication checks unsigned magnitudes before multiplying, avoiding a hidden
compiler-rt dependency.

## Host wire format

Exports are `pt_abi_version`, `pt_run`, `pt_output_ptr`, `pt_output_len`,
`pt_trap_cause`, `pt_collections`, and `memory`. `pt_run` returns a trap code or
zero. The output buffer contains a tag byte; Bool/I64 use eight little-endian
bytes, strings have a u32 UTF-8 byte length, and aggregate values have a u32 child
count followed by recursively encoded values. Variants add a u32 local symbol
identity; records prefix each child with a u32 field identity. Identities must be
interpreted with the checked program used to build the artifact. Unit has no
payload. The host validates tags, bounds, UTF-8, boolean values, record uniqueness,
recursion depth, and absence of trailing bytes. Dropping the Wasmi store reclaims
all module state on every exit path. The default host owns no external resources;
the opt-in quota host revokes its invocation on every exit.

## Regression validation

The routine backend suite compiles and executes all 48 recorded pure conformance
cases, checks generic records and nested Unicode patterns, forces collection with
a 2 KiB heap, exercises 24 seeded arithmetic inputs, checks fuel thresholds and
I64 minimum multiplication, runs 500 generated malformed frontend inputs, and
rejects 256 malformed modules plus an oversized-output guest. Pact tests separately
compare all 17 recorded guarded cases. These are deterministic bounded regression
and fuzz-smoke tests, not a claim of exhaustive fuzzing or formal backend proof.
The fixed MIN/-1 multiplication regression preserves the compiler-rt linking
failure found while adding this backend.

## Opt-in quota capabilities

`emit_c_quota` emits guarded C for registered `host::quota::Txn` parameters and
`read`, `reserve`, `commit`, and `rollback` calls. The actual compiled example is
[`examples/quota/src/transaction.ptn`](../examples/quota/src/transaction.ptn).
Its entry arguments use `QuotaArgument::Value` for ordinary values and
`QuotaArgument::HandleSlot` for a root-grant selector. Source code cannot construct
a handle. Ordinary `emit_c`/`run_wasm` keep their deny-all-import behavior.

`run_wasm_quota` takes ownership of a Rust `host::Invocation` and a vector of real
`host::Handle` tokens previously issued by that invocation. It permits only these
`pactern.quota.v1` function imports with exact Wasm signatures:

| Import | Signature | Behavior |
| --- | --- | --- |
| root | `(i32) -> i32` | Consume one supplied root slot, return a guest token |
| read | `(i32) -> i64` | Borrow token to read its transaction |
| reserve | `(i32, i64) -> i32` | Borrow token, reserve a positive amount, return Bool |
| commit | `(i32) -> ()` | Consume token and commit the optimistic transaction |
| rollback | `(i32) -> ()` | Consume token and discard pending changes |
| release | `(i32) -> ()` | Consume token for unmetered implicit scope cleanup |

Import names, namespaces, kinds and signatures are checked before execution.
Guest tokens refer only to the provided root grants; guessing an integer cannot
create new authority. Each root slot can be claimed once, and a committed,
rolled-back or released token cannot be reused. A cap of 1024 root handles bounds
host token maps. Token values use guest value tag 7, are not GC pointers, and cannot
appear in output. The static affine checker prevents source-level copying and
escape; the host independently enforces liveness, scope and budgets for malicious
Wasm that bypasses the compiler.

The checked IR preserves borrowed parameter modes. An owning handle read moves
its token and clears the old slot. Borrowed calls retain the caller's slot, and
borrowed helper parameters never release the caller's authority. Block locals are
released on fallthrough, return, break and continue; function cleanup releases
remaining owning parameters and locals. `release` has no external write and no
operation/byte charge, so scope cleanup still works with an exhausted budget.
It is not recorded as an external interaction; replay makes the same deterministic
liveness change. Explicit source `rollback` remains a recorded, budgeted operation.

Host execution uses resumable Wasmi calls in 10,000-fuel slices. Between slices,
and before/after execution, a host checkpoint checks cancellation and every active
deadline without consuming operation budget. A single operation needing more fuel
can use a larger slice within the total fuel limit. Initialization has a 10,000-fuel
cap. Module validation/translation is eager and bounded by module byte limits,
with deadline checks before and after it. Cancellation never depends on a guest
voluntarily calling a checkpoint. Wasm memory limits remain independent of the
invocation's operation, byte and host-memory budgets.

The returned `QuotaExecution` separates `outcome: Result<Execution, BackendError>`
from `trace: Result<Trace, HostError>`, preserving recorded interactions after a
language or host trap. Replay finalization rejects unread trace tails. Every exit
revokes the invocation and drops pending transactions; explicit commits completed
earlier remain committed. Replay performs zero live writes. This API is for a
trusted Rust embedding application; the CLI does not mint handles from integers
or automatically grant storage access.

```sh
cargo test -p pactern --locked --test backend_quota
```

The bridge suite executes Pactern borrowing, reservation, commit and rollback;
tests operation budgets, asynchronous cancellation and timeout of an infinite
guest loop; rejects forged/stale tokens, repeated roots and spoofed imports; and
checks implicit ownership cleanup, tenant isolation and zero-write replay. Separate
adversarial tests cover start functions, memory limits, imported-memory spoofing,
foreign handles and redacted traces.
