# Design: ZIR — the Zap typed intermediate representation

- **Author(s):** Zap team
- **Last updated:** 2026-10-18
- **Status:** Accepted (design) — implementation staged (refactor of `crates/codegen`)
- **Discussion:** github.com/parveshgarg/zap#12

## Abstract

ZIR is Zap's **typed, SSA-based mid-level IR**, sitting between the
type-checked AST and the code generators. It is the frozen contract that lets
the front-end and the backend(s) evolve independently and that unblocks
conflict-free parallel work on feature codegen: once ZIR is stable, each
feature lowers AST → ZIR and ZIR → backend separately, rather than everyone
editing one monolithic `codegen/lib.rs`.

## Background

The Phase 1 vertical slice lowers the AST **directly** to LLVM IR in
`crates/codegen/src/lib.rs`. That was the right call to get `fib`/`loop_sum`
running, but it means every new feature touches the same file, and it ties the
compiler to one backend. The ROADMAP's parallelization map calls for freezing
the typed-IR schema early. ZIR is that schema. It also gives us:

- a stable target the feature tickets (strings, structs, collections,
  exceptions, arrays) lower *into*, independently;
- a home for backend-independent optimization passes;
- a second backend (e.g. Cranelift for fast debug builds) as a drop-in behind
  the existing `Backend` trait, consuming the same ZIR.

## Proposal

### Shape

A ZIR **module** is a set of:

- **type definitions** (named struct/aggregate layouts),
- **function declarations** (external/runtime) and **definitions**,
- **global constants** (e.g. interned string data).

A **function** is: a name, a typed parameter list, a return type, and a body —
an ordered list of **basic blocks**. The first block is the entry.

### Types

ZIR types are lower-level than Zap types (which lower *into* them):

| ZIR type | From Zap |
|---|---|
| `i1`, `i8`, `i16`, `i32`, `i64` | `bool`, `char` (i32), `int32`, `int64` |
| `f32`, `f64` | `float32`, `float64` |
| `ptr` | all reference types (opaque pointer; see RUNTIME-ABI) |
| `struct { ... }` / named aggregate | `struct` instances' *layout* (allocated behind a `ptr`) |
| `void` | no-value / statement result |

Reference types (`str`, `list`, `dict`, `array`, struct instances) are a `ptr`
to a heap object whose layout is defined by `design/RUNTIME-ABI.md`. ZIR itself
stays layout-agnostic: it manipulates `ptr`s and calls runtime entry points.

### Values & SSA

ZIR is **SSA**: every instruction defines at most one value, each value is
assigned exactly once, and values are typed (`%3 : i64`). Control-flow merges
use **block parameters** (as in Cranelift/MLIR) rather than `phi` nodes — a
block declares typed parameters and branches pass matching arguments:

```
block loop(%i: i64):
    ...
    br_if %cond, loop(%i_next), exit()
```

Mutable Zap variables are initially lowered via **stack slots**
(`alloca`/`load`/`store`); a later mem2reg-style pass can promote them to block
parameters. (The current direct-to-LLVM backend already uses stack slots, so
this matches reality and offers an optimization path, not a rewrite.)

### Instructions (core set)

- **Constants:** `iconst.T`, `fconst.T`, `bconst`.
- **Integer arith:** `iadd isub imul sdiv srem` and bitwise `and or xor shl ashr`.
- **Float arith:** `fadd fsub fmul fdiv frem fneg`.
- **Compare:** `icmp.<pred>` (`eq ne slt sle sgt sge`), `fcmp.<pred>`.
- **Conversions:** `sext trunc`, `sitofp fptosi`, `fpext fptrunc`.
- **Memory (stack slots):** `alloca T`, `load T, %p`, `store %p, %v`.
- **Aggregates:** `gep` (compute field/element address), or high-level
  `field.get`/`field.set` that lower to `gep`+`load`/`store`.
- **Calls:** `call @f(args...) -> T`, `call_indirect %fp(args...)`,
  and `call_runtime @zap_*` for runtime entry points (RUNTIME-ABI).
- **Control flow:** `br block(args)`, `br_if %c, then(args), else(args)`,
  `ret %v?`, `unreachable`.
- **Exceptions (#22):** `invoke` (a call with a normal path and an `unwind`
  edge to a landing-pad block) and `landingpad` (a landing-pad block's entry,
  yielding the in-flight exception object). See RUNTIME-ABI §"Exception /
  unwinding ABI".
- **Reserved (later):** `gc.safepoint` (see Open issues and RUNTIME-ABI).

### Example: `fib` in ZIR

```
func @fib(%n: i64) -> i64 {
  entry:
    %t0 = iconst.i64 2
    %c  = icmp.slt %n, %t0
    br_if %c, ret_n(), rec()
  ret_n:
    ret %n
  rec:
    %one = iconst.i64 1
    %a   = isub %n, %one
    %r1  = call @fib(%a) -> i64
    %two = iconst.i64 2
    %b   = isub %n, %two
    %r2  = call @fib(%b) -> i64
    %s   = iadd %r1, %r2
    ret %s
}
```

A mutable loop (`loop_sum`) lowers its `total`/`i` to `alloca i64` stack slots
with `load`/`store`, and the `for`-over-`range` becomes the cond/body/incr
blocks the current backend already emits.

### Lowering AST → ZIR (sketch)

- Each Zap function → a ZIR function; `main` → `@main -> i32`.
- Blocks/statements → basic blocks + straight-line instructions; `if`/`while`/
  `for`/`do-while` → branches; `return` → `ret`.
- Expressions → SSA value chains; calls → `call`; `print` → `call_runtime`.
- Zap types → ZIR types via the table above; reference types stay `ptr` and go
  through runtime calls.

### ZIR → LLVM mapping

Nearly 1:1: ZIR instructions map to the corresponding `inkwell` builder calls;
**block parameters → LLVM `phi` nodes**; stack slots → `alloca`. The existing
LLVM backend becomes "ZIR → LLVM" instead of "AST → LLVM". A second backend
(Cranelift) can consume the same ZIR.

### Textual form & tooling

ZIR has a printable textual form (as above) for debugging and tests, exposed as
`zapc --emit ir <file.zap>`. Golden IR tests can pin lowering output.

## Rationale

- **A typed SSA IR** is the standard way to decouple front-end from back-end,
  enable optimization, and support multiple targets — and it is exactly the
  contract the ROADMAP says must be frozen to parallelize.
- **Block parameters over `phi`** are easier to produce and manipulate and map
  cleanly to `phi` at LLVM lowering.
- **Hybrid stack-slots + SSA** matches the current backend (no rewrite) while
  leaving an optimization path (mem2reg → block params).
- **`ptr` + runtime calls for reference types** keeps ZIR layout-agnostic; all
  layout lives in `design/RUNTIME-ABI.md`, so the two contracts compose.

## Compatibility

Additive design. The implementation is a **refactor** of `crates/codegen` from
"AST → LLVM" to "AST → ZIR → LLVM"; it does not change language semantics or
the `Backend` trait's role. Feature tickets written against ZIR will not
conflict with each other.

## Implementation

Staged, as a follow-up to this doc:

1. New crate `crates/ir` (`zapc_ir`): the ZIR data structures + textual printer.
2. `crates/lower` (or a module): AST → ZIR, reproducing the current slice.
3. Refactor `crates/codegen` to ZIR → LLVM; keep the `Backend` trait.
4. `zapc --emit ir`; golden IR tests.
5. Feature tickets (#17, #18, #21, #22, #24 …) then target ZIR.

Phase boundary: **this is the front-end ↔ back-end contract.** It pairs with
`design/RUNTIME-ABI.md` (the compiled-code ↔ runtime contract).

## Open issues

- **Exception representation:** `invoke`/`landingpad`-style edges vs. explicit
  cleanup blocks — resolve with #22 and RUNTIME-ABI.
- **GC safepoints / stack maps:** not needed for conservative Boehm GC; required
  for precise GC later — reserve `gc.safepoint` now, specify later.
- **When to adopt block-parameter promotion** (mem2reg) vs. staying on stack
  slots — correctness first, optimization later.
- **Generics:** monomorphization happens before/into ZIR (specialized functions)
  — confirm with the generics design (#15).
- **Name mangling** for monomorphized/overloaded symbols.
