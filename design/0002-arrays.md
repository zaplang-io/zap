# Proposal: Built-in N-D numeric arrays with SIMD

- **Author(s):** Zap team
- **Last updated:** 2026-10-17
- **Status:** Accepted (design) — implemented incrementally
- **Discussion:** (internal) — see `DECISIONS.md` D-0011

## Abstract

Zap includes a built-in dense numeric array type with an N-dimensional
**shape** — NumPy-like arrays available with no `import`. Element-wise
arithmetic (`a + b`), broadcasting, reductions, and slicing are language/stdlib
built-ins, and the compiler lowers element-wise operations to SIMD instructions
when the target supports them.

## Background

The positioning is "Python devs suffering from performance." NumPy is the
reason most of them reach for native speed, so making dense numeric arrays and
vectorized math a *default* construct — not a library — is a core
differentiator. This type is distinct from `list[T]`:

- `list[T]` — generic, growable, heterogeneous-capable container.
- `array[T]` (aka tensor) — dense, contiguous, fixed-dtype, shaped, vectorized.

## Current Experimental contract (I#476)

The normative supported subset is SPEC §15: **N-dimensional** arrays with a
**static rank** (`ndim`, part of the type), constructed from a flat or nested
**rectangular** list literal; the full dtype matrix — floats `f32`/`f64`, signed
`i8`/`i16`/`i32`/`i64`, unsigned `u8`/`u16`/`u32`/`u64`, and `bool`; integer
indexing per axis (a full index `m[i, j]` is a scalar get/set, a partial index
`m[i]` a rank-reduced view); `a.ndim`/`a.shape`/`a.size`, `len`, `sum` (over all
elements), element-wise `+`/`-`/`*`, scalar broadcasting (`+`/`*` on either side,
`-` with array on the left), **array-array broadcasting** (right-align shapes,
stretch size-1 axes, extend rank), **NEP-50 dtype promotion**, and contiguous
1-D view slices (optional literal step `1`).
Shapes that cannot broadcast together raise `ValueError` before entering the
kernel. Empty and zero-axis arrays are supported.

Axis reductions, N-D and strided views, `sqrt` (and other elementwise math),
division/comparison kernels, broadcasting in assignment / partial-index
(whole-row) stores, inner-axis SIMD on the broadcast path, and
list-variable/iterable `array[T]` construction are future design work. The broader examples
below describe that future design rather than an available surface. Unsupported
forms receive semantic or lowering errors.
`float64`/`float` is supported in every built-in collection position — list
elements (I#569), dict values/keys, set members, and heap/channel elements (#800
Phase 1 D-0137, #800 Phase 2 / #803 D-0138; a float key/member canonicalizes
`-0.0` and rejects `NaN` with a catchable `ValueError`). Only `f32`/`float32`
collection elements remain deferred (#49). Float fields inside tuples/structs
remain valid. Dense numeric float buffers use `array[f64]`.

## Proposal (future N-D design)

### Type

- `array[T]` where `T` is a numeric dtype: `f32`, `f64`, `i32`, `i64`
  (dtype set may grow). An array carries a **shape** (tuple of dims) and is
  stored as a single contiguous buffer (row-major).
- Construction:

```python
auto a = array[f64]([1, 2, 3])                 # shape (3,)
auto m = array[f64]([[1, 2], [3, 4]])          # shape (2, 2)
auto z = zeros[f64]((3, 4))                     # shape (3, 4)
auto r = arange[i64](0, 10)                     # shape (10,)
print(m.shape)                                  # (2, 4)-style tuple
```

### Element-wise operators (overloaded for `array`)

```python
auto c = a + b            # element-wise, same shape
auto d = a * 2.0          # scalar broadcast
auto e = a / b + 1.0
auto f = sqrt(a)          # elementwise intrinsics
auto s = sum(a)           # reduction -> scalar
auto s0 = sum(m, axis=0)  # reduction along an axis
```

### Broadcasting

NumPy-style broadcasting (align trailing dims; size-1 dims stretch). Scope is
introduced incrementally — see Implementation.

### Slicing / indexing

```python
auto row = m[0]           # first row
auto col = m[:, 1]        # NumPy-style slice
auto sub = m[0:2, 1:3]
m[0, 0] = 9.0
```

Shape mismatches in element-wise ops raise a runtime exception (D-0007); these
are "bug"-class errors, so an uncaught one aborts with a trace.

### SIMD

SIMD is a **codegen/runtime** concern, not surface syntax:

- Element-wise kernels lower to LLVM vector operations; the scalar tail is
  handled with a remainder loop.
- Target features come from a compile-time `-mcpu`/target flag, with optional
  **runtime CPU feature dispatch** (pick an AVX2/AVX-512/NEON kernel at load
  time).
- This layers on the lower-level `vec[T, N]` SIMD primitive (a fixed-width lane
  vector), which remains available for hand-written kernels.

## Rationale

- **Built-in, not a library:** removes the `import numpy` friction and lets the
  compiler own vectorization and layout, which a library cannot do as well.
- **Separate from `list[T]`:** dense numeric storage needs a fixed dtype and
  contiguous layout for SIMD; conflating it with the generic container would
  cost performance and clarity.
- **Full N-D with shape, built incrementally:** committing to N-D up front keeps
  the type system and operator semantics coherent, while implementation ships
  1-D first to get value early (see below).

## Compatibility

New built-in type and overloaded operators for it. Operator overloading is, for
now, **restricted to built-in types** (no user-defined operator overloading
yet) to keep the type checker simple — revisit later.

## Implementation

Roadmap **Phase 3, Arrays track** (design) with staged delivery:

1. **1-D `array[T]`**: construction, element-wise ops, scalar broadcast, basic
   reductions (`sum`, `min`, `max`), SIMD lowering for the element-wise kernels.
2. **N-D shape**: shape metadata, row-major indexing, row/axis access.
3. **Broadcasting**: trailing-dim alignment + size-1 stretch.
4. **Slicing**: NumPy-style multi-axis slices and views (decide copy-vs-view
   semantics — see Open issues).
5. **Kernel library + runtime dispatch**: optimized per-ISA kernels.

Phase boundaries touched: codegen (SIMD lowering) and the runtime ABI (array
header + buffer layout, GC-managed buffer).

## Implementation status

**Increment 1 (shipped, ticket #24):** 1-D `array[f64]` and `array[i64]`.

- Construction from a list literal: `array[f64]([1.0, 2.0, 3.0])`.
- Indexing `a[i]` (get/set), `len(a)`.
- Element-wise `+`, `-`, `*` on same-length arrays of the same dtype, and
  scalar broadcast (`a + 2.0`, `a * 2`, and commutative `2.0 + a`).
- Reduction `sum(a)` → scalar.
- **SIMD:** the element-wise kernels lower to explicit LLVM vector
  load/arith/store (`<4 x double>` / `<4 x i64>`) with a scalar remainder loop
  (codegen `Inst::ArrayBinOp`). The array object is a GC-managed contiguous
  buffer per `design/RUNTIME-ABI.md` (dtype + ndim + dims + data), allocated
  pointer-free (`GC_MALLOC_ATOMIC`).

**Increment 2 (shipped, ticket #49):** dtype promotion + `uint8`/`bool` dtypes.

- **Dtype promotion** for mixed-dtype element-wise ops. The promotion lattice
  is `bool < uint8 < i64 < f64`; two array operands promote to the
  higher-ranked dtype (`i64 + f64 -> f64`, `uint8 + i64 -> i64`). Arithmetic
  never *yields* `bool` — `bool + bool -> uint8` (`True + True == 2`), matching
  Python's `bool`-as-`int`. A **scalar is weak** (NumPy NEP 50): an integer
  scalar keeps the array's dtype (`uint8_image + 1` stays `uint8`); only a float
  scalar forces `f64`. The SIMD kernel loads each operand at its own storage
  type, widens it to the result dtype (zext/sext/uitofp/sitofp, vectorized),
  computes in the result dtype, and stores.
- **`uint8`** (image data) and **`bool`** (masks) dtypes: 1-byte storage words
  (vs the 8-byte `f64`/`i64`). `zap_array_new` sizes the buffer by dtype;
  `uint8`/`bool` elements cross the runtime ABI as `i64` words (get
  zero-extends, set truncates to a byte / normalizes to `0|1`). Integer-array
  arithmetic wraps per D-0005 (`uint8` wraps mod 256).

**Increment 3 (shipped, #500 / D-0164):** N-D shape + multi-axis indexing.

- `array[T]` carries a **static rank** (`ndim`) in its type. A nested
  rectangular list literal builds a shaped array (`array[f64]([[1, 2],
  [3, 4]])` -> shape `(2, 2)`); ragged literals are a compile error. The runtime
  allocates the rank-`n` header with `zap_array_new_nd` + per-axis
  `zap_array_set_dim`.
- Indexing: a **full** index `m[i, j]` reaches a scalar (get/set, incl. `+=`); a
  **partial** index `m[i]` is a contiguous rank-`(n-1)` **view** that aliases the
  parent (`zap_array_row`: shares the buffer base, advances `off` by a
  row-stride, copies `dims[1..]`). `m[i, j]` lowers as a fold of row views down
  to a rank-1 view plus a 1-D get, so the narrow-dtype get/set are reused
  unchanged. Out-of-range aborts (as 1-D); negative indices stay unsupported.
- Shape surface: `a.ndim` (constant), `a.shape` (a `tuple[int64; ndim]` from
  `zap_array_dim`), `a.size` (`zap_array_count`); `len(a)` is the first axis.
- Element-wise ops require equal rank and preserve it; `zap_array_check_shape`
  compares the full shape; the result buffer is `zap_array_new_like` (same shape,
  result dtype). `sum(a)` reduces over the whole element count. The SIMD kernel
  is unchanged — it runs flat over `zap_array_count` elements.

**Increment 4 (shipped, #501 / D-0165):** element-wise broadcasting.

- `+`/`-`/`*` on two arrays broadcast (NumPy): shapes right-align, a size-1 or
  missing (lower-rank) axis stretches, the result takes the larger rank and the
  larger aligned dim. Incompatible shapes raise a catchable `ValueError` from a
  throwing `zap_array_broadcast_check` emitted before the kernel.
- The kernel stays in codegen and branches on `zap_array_same_shape`: equal
  shapes keep the existing **flat SIMD** loop (no regression); otherwise a
  **strided** loop walks the result row-major and maps each output index to each
  operand's window-relative offset via `zap_array_bcast_offsets` (stretched axes
  → stride 0). The result is allocated by `zap_array_broadcast_new`. Inner-axis
  SIMD on the broadcast path is a deferred perf follow-up (scalar body for now).

**Increment 5 (shipped, #503 / #890 / D-0166):** the full dtype matrix.

- Dtypes: floats `f32`/`f64`, signed `i8`/`i16`/`i32`/`i64`, unsigned
  `u8`/`u16`/`u32`/`u64`, `bool`. Each integer crosses the get/set/sum ABI as an
  `int64` word (sign/zero-extend on get, wrap on set); each float as a `float64`
  word (widen/narrow). Storage is 1/2/4/8 bytes; the SIMD kernel loads/computes
  at each width (new `i16`/`i32` int and `f32` fpext/fptrunc `convert` cases).
  Runtime get/set/sum are macro-generated per dtype (`ZAP_ARRAY_INT_ACCESS` /
  `ZAP_ARRAY_FLOAT_ACCESS`); dtype codes 0..=3 are preserved, 4..=10 are new.
- Promotion is **NEP-50** (`Dtype::promote`): widest float wins and an int never
  bumps `f32`→`f64`; same-signedness integers take the wider; mixed
  signed/unsigned take the signed type that holds both (`u8+i8→i16`,
  `u32+i32→i64`), `u64`-with-signed → `f64`. Weak scalars keep the array dtype
  (int) or a float array's width (float).

**Increment 6 (shipped, #504 / D-0167):** constructors.

- `zeros[T](shape)` / `ones[T](shape)` / `full[T](shape, value)`: `shape` is an
  int (1-D) or a tuple literal (N-D) — arity = static rank, dim values runtime.
  Lowering allocates via `zap_array_new_nd` + `zap_array_set_dim` and fills with
  `zap_array_fill_i`/`_f`.
- `arange[T](start, stop[, step])`: half-open 1-D range, runtime length; a zero
  step raises `ValueError` (`zap_array_arange_i`/`_f`). `linspace[T](start, stop,
  num)`: `num` samples inclusive of both ends (`zap_array_linspace`). All over
  the full dtype matrix.

**Increment 7 (shipped, #505 / D-0168):** element-wise math intrinsics.

- The `math` module's unary functions map over an array argument:
  `math.sqrt`/`exp`/`log`/`sin`/`cos`/`tan`/`fabs`. Result is a float array of
  the same shape (`f32`→`f32`, else `f64`); scalar `math.<fn>` is unchanged.
- A new `Inst::ArrayUnaryOp` + unary SIMD codegen kernel (load → convert to the
  float result dtype → LLVM math intrinsic → store; `tan` = `sin`/`cos`). No new
  runtime entry points. The other `math` unary fns reject an array argument for
  now; `pow`/binary math over arrays is deferred.

**Increment 8 (shipped, #506 / D-0169):** division + non-commutative
scalar-on-left.

- `/` completes the element-wise operators as NumPy **true division**: the
  result dtype is always a float, so an all-integer `/` lifts to `f64`
  (`array[i64] / 2 → f64`, `array[i8] / array[i8] → f64`) while a float operand
  keeps its width under NEP-50 (`array[f32] / 2 → f32`). A zero divisor follows
  IEEE-754 (`x/0 → ±inf`, `0/0 → nan`) and does **not** raise — array ops follow
  NumPy (warn, don't raise), and a per-element zero check would serialize the
  SIMD kernel. This is a deliberate divergence from scalar `/`, which raises a
  catchable `ZeroDivisionError`.
- Scalar-on-the-left now lowers for every op. The commutative `s + a` / `s * a`
  reorder to `a + s` / `a * s`; the non-commutative `s - a` / `s / a` are kept
  as scalar-on-left so the kernel computes `s <op> a[i]`.
- Reuses `Inst::ArrayBinOp` (now `op: Div` + a `scalar_lhs` flag) and the
  existing vector-body + scalar-remainder kernel; the scalar-on-left form swaps
  the two per-lane arithmetic operands, and division adds only a float-divide
  arm (`out` is always float, so no integer-divide path). No new runtime entry
  points; the ABI is unchanged. Floor division `//` and `%` over arrays stay
  deferred.

**Increment 9 (shipped, #507 / D-0170):** reductions + axis.

- `a.sum()`/`a.min()`/`a.max()`/`a.mean()`/`a.prod()` as array **methods**, each
  with an optional `axis=`. Full reductions keep the dtype (`mean` is a float:
  `f32` source → `f32`, else `f64`); `axis=k` collapses one axis to a rank-(n-1)
  array (on a rank-1 array the scalar reduction). Negative axis normalized at
  runtime; out-of-range `axis` raises `ValueError`. `keepdims` deferred.
- Empty: `sum → 0`, `prod → 1`, `mean → nan`; `min`/`max` on an empty array/axis
  raise a catchable `ValueError`, so those forms lower through `throwing_call`.
- No codegen/SIMD change: reductions lower to runtime calls, like `sum`. The
  per-dtype full reducers are macro-generated beside get/set/sum; the axis forms
  are `zap_array_reduce_axis(arr, axis, op)` (sum/min/max/prod, result dtype =
  input) and `zap_array_mean_axis(arr, axis)` (float result), factoring the
  contiguous buffer into `outer × axislen × inner`. The array header ABI is
  unchanged.

**Increment 10 (shipped, #502 / D-0171):** strided views — the full N-D stride
model.

- `ZapArray` gains `strides` (NULL == contiguous row-major; non-NULL == a view
  with one signed element stride per axis). The precise collector forwards it
  like `dims`/`data`.
- Views that alias the parent: `a[::2]` / `a[::-1]` (1-D stepped / reversed),
  `a[lo:hi:step]`, `m[lo:hi]` (N-D axis-0, rank-preserving), and a sliced axis in
  a multi-axis index `m[:, j]` / `m[lo:hi, k]` / `m[i, ::2]`. Scalar get/set are
  stride-aware (so a write aliases the parent); `zap_array_index_axis` /
  `zap_array_slice_axis` build the views and a multi-axis index folds them left
  to right. Assigning through a sliced index (a strided-view store) is deferred.
- The SIMD kernels and reductions stay contiguous: each calls `zap_array_contig`
  at entry, which compacts a strided operand to a fresh contiguous copy first
  (read-only, so no aliasing is lost). NULL-is-contiguous keeps the common path
  and every existing kernel untouched.
- Parser/AST: a baseless `Expr::SliceSpec` carries a slice as one element of a
  subscript tuple (`m[:, j]`); a lone slice still parses to `Expr::Slice`.

**Increment 11 (shipped, #509 / D-0172):** matrix multiply `@` / `dot`.

- The infix `@` (PEP 465, `*` precedence) + in-place `@=` + `dot(a, b)` on 1-D
  and 2-D arrays: 1-D·1-D → scalar dot, 2-D·2-D → matrix, mixed 1-D/2-D → a 1-D
  vector (row/column, singleton axis dropped). NEP-50 result dtype; a contracted-
  axis mismatch raises `ValueError`. Batched (≥3-D) matmul deferred.
- No special-casing: a new `ast::BinOp::MatMul` with the `__matmul__` /
  `__imatmul__` dunder mapping (D-0047), so a user struct's `@` works too; arrays
  are intercepted in lowering (like `+`/`/`) and emit the runtime kernels. No IR
  `BinOp` / codegen change. Runtime: `zap_array_dot{i,f}` (scalar) and
  `zap_array_matmul(a, b, out_dtype)` (array), compacting operands via
  `zap_array_contig`. No array-header ABI change.

**Increment 12 (shipped, #511 / D-0174):** shape ops.

- `a.reshape(d…)` (view-or-copy, one `-1` inferred), `a.transpose()` / `a.T`
  (view, reverse axes), `a.flatten()` (1-D copy) as methods/attribute;
  `concatenate([…], axis=)` / `stack([…], axis=)` as builtins over a list literal
  of arrays (fresh copies; concat keeps rank, stack adds one). Shape/size/axis
  faults raise a catchable `ValueError`.
- transpose is pure metadata (reverse dims + effective strides); the rest reuse
  `zap_array_contig` (#502) then copy. `concatenate`/`stack` require a list
  literal (the `list[array[T]]` element type erases dtype/rank in the collection
  ABI) and read rank/dtype from the first element. No new grammar; no array-ABI
  change.

**Increment 13 (shipped, #510 / D-0175):** boolean-mask & fancy (integer-array)
indexing + `where` — the final feature increment of #499.

- Element-wise comparisons `== != < <= > >=` → `array[bool]` masks (broadcast +
  scalar), `& | ^` combine and `~` inverts; array truthiness / `and` / `or` /
  chained `lo < a < hi` raise NumPy's ambiguous `ValueError`.
- `a[mask]` (leading-axis boolean, result rank `1 + ndim − mask.ndim`) and
  `a[idx]` (integer gather along axis 0, result `idx.shape + a.shape[1:]`) are
  fresh **copies**; `a[mask] = …` / `a[idx] = …` scatter in place (last write
  wins). The general `a[p0, p1, …]` engine mixes scalars, slices, and index
  arrays, broadcasts the advanced indices, and places the broadcast axes by
  NumPy's transpose rule (at the advanced block when contiguous, else at the
  front). `where(cond, x, y)` selects with full broadcasting; `where(cond)` is
  `nonzero` (a tuple of index arrays).
- All runtime C (the matmul/reductions pattern), not inline SIMD: compare/logical
  kernels, `mask_select`/`mask_assign_*`, `take`/`scatter_*`, the general
  `zap_array_fancy` (spec-buffer + index-array list), `where`/`nonzero_axis`, and
  a general N-D `zap_array_phys_nd` for in-place advanced assignment. Comparisons
  stay loose (`Unknown`) in sema so masks combine and index. No new grammar; no
  array-ABI change. Matches NumPy 2.5.4 (verified before implementation).

**Increment 14 (shipped, #911 / D-0177):** vectorized reduction kernels — the
first performance increment (epic #914, Phase 1; sibling of the broadcast-kernel
work #896).

- `sum`/`min`/`max`/`mean`/`prod` (whole-array and `axis=`, #507/D-0170) now run
  an eight-lane multi-accumulator body + scalar tail instead of a single
  latency-bound scalar chain, so the compiler packs the lanes into SIMD
  (bandwidth-bound target ~4–8×, #909 — the FLOOR kernel, simple portable C, no
  intrinsics, tuning deferred).
- Behaviour unchanged. Integer reductions are bit-identical at any size; an
  `axis=` reduction with a contiguous last axis reuses the unrolled reducer,
  while a non-last axis interchanges the reduced/contiguous loops to vectorize —
  both keeping each cell's per-element accumulation order, so every `axis=`
  reduction is bit-identical for all dtypes. Only a floating-point whole-array or
  last-axis `sum`/`prod`/`mean` reassociates (within IEEE-754 tolerance).
  Accumulator widths are the D-0170 ones (int sum → int64, float → double).

**Increment 15 (shipped, #896 / D-0178):** vectorized broadcast element-wise
kernel — the broadcast half of epic #914 Phase 1 (sibling of the reduction work
#911).

- The broadcast path (`m + row`, `m + col`, outer products, N-D, mixed dtype) was
  a per-element scalar loop calling `zap_array_bcast_offsets` for every element;
  it now walks the result one inner row at a time (one offset call per row) and
  vectorizes the innermost axis with a `<4 × T>` body + scalar tail, matching the
  equal-shape flat kernel. Inline in codegen (the mixed-dtype/NEP-50 arithmetic
  lives there, not the runtime).
- Each operand is compacted/contiguous, so its inner step is either its stride
  (contiguous → vector-load) or 0 (stretched size-1 axis → constant across the
  row, splat into the lanes); no gather case arises. The kernel is specialized on
  (lhs contiguous?, rhs contiguous?) so the hot loop has no per-lane branch.
  Results are unchanged. New runtime helper `zap_array_bcast_inner_stride`;
  `zap_array_bcast_offsets` reused.

**Increment 16 (shipped, #912 / D-0179):** numeric acceleration seam — epic #914
Phase 2 (infrastructure; the matmul dispatch that uses it is #904).

- A provider-neutral seam (`crates/codegen/src/zapblas.inc.c`) the dispatcher
  consults: capability probe `zap_accel_available()` / `zap_accel_ops()`, the tier
  knob `zap_accel_tier()` (`ZAP_ACCEL=auto|native|fast`), and the thread budget
  `zap_accel_ensure_threads()` (pins the provider pool to `ZAP_ACCEL_THREADS` or
  the container-aware MAXPROCS, #905).
- Provider chosen at build time — OpenBLAS (Linux, `pkg-config`) / Apple
  Accelerate (macOS) — and linked only for a program that reaches a `zap_accel_*`
  seam, exactly like the pcre2/zlib backends (#853): codegen gates
  `-DZAP_HAVE_BLAS` + the provider link flags on `BackendUse::blas`. `ZAP_NO_BLAS`
  opts out; no provider ⇒ `fast` degrades to `native`. **Dormant until #904**
  wires the first caller, so no program links or runs BLAS yet.

**Increment 17 (shipped, #913 / D-0180):** own blocked+SIMD matmul floor kernel —
epic #914 Phase 2 (the FLOOR the dispatch #904 picks for small / integer /
no-BLAS cases).

- `@` / `dot` (#509/D-0172) was a scalar triple loop reading each element through
  a per-element dtype switch and walking B down a column (stride `n`). It now
  materializes A/B into contiguous accumulator-typed buffers once and runs the
  cache-friendly `ikj` order whose inner loop is a vectorized contiguous SAXPY
  (`zap_matmul_f` / `zap_matmul_i` in `zaprt.c`; the `zap_array_matmul` entry
  point and lowering are unchanged).
- Simple and portable, **tuning deferred** (#909: the floor deliberately does not
  chase BLAS's register-tiled microkernel). Bit-identical to the scalar kernel
  for every dtype and shape — each output cell keeps its contraction order, so no
  fp reassociation. 1-D·1-D `dot` keeps its scalar accumulator.

**Increment 18 (shipped, #904 / D-0181):** matmul auto-dispatch — own floor vs
BLAS gemm. Completes epic #914 Phase 2 (the consumer that wires the #912 seam +
#913 floor).

- `zap_array_matmul` dispatches: integer → always own (exact); float → BLAS
  (`cblas_{d,s}gemm`) when a provider is linked and (`ZAP_ACCEL=fast`, or `auto`
  past the hardcoded ~`n≥64` floor), else the own floor; `ZAP_ACCEL=native`
  forces own. The BLAS pool is pinned to the container-aware MAXPROCS (#905) via
  `zap_accel_ensure_threads`, overridable by `ZAP_ACCEL_THREADS`.
- A program that uses `@`/2-D `dot` now flips `BackendUse::blas`, so the runtime
  compiles with the provider and links it (pcre2/zlib pattern) — this is the
  first caller that actually exercises the #912 seam. BLAS reassociates, so a
  large float `@` matches the own kernel within fp tolerance (NumPy-like);
  integers stay exact. Floor is tunable (#904 benchmarking), not an API promise.

**Deferred (clean follow-ups, noted so they are not silently missing):**

- Integer-array (non-mask) bitwise element-wise ops (`&`/`|`/`^`/`~` on numeric
  arrays — a sibling of the Increment 7/8 element-wise ops, out of #510's
  indexing scope); `ravel` (a view-when-possible flatten); `add.at`-style
  accumulating scatter (#510 uses last-write-wins, as NumPy's plain `a[idx]=`).
- **Matmul performance** (#904, D-0173/D-0181): **shipped** (Increments 16–18 —
  the #912 seam, the #913 own floor, and the #904 auto-dispatch). Remaining
  tuning follow-ups: a register-tiled + packed own micro-kernel targeting ~70–90%
  of single-thread BLAS, and refining the hardcoded BLAS floor after the
  1/4/32-core ±container benchmarking (#904; RFC = discussion #906).
- Broadcasting in assignment / partial-index (whole-row) stores `m[i] = row`
  (#501 follow-up / #511). (Inner-axis SIMD for the broadcast path shipped in
  Increment 15, #896 / D-0178.)
- Assigning *through* a sliced index — a strided-view store `m[:, j] = col` —
  stays a lowering error (the read side ships in Increment 10 below).
- Floor division `//` and modulo `%` over arrays (a lowering error for now; the
  existing kernel extends to them with an integer `sdiv`/`udiv` + floor-adjust).
- `keepdims` reductions and multi-axis `axis=(i, j)`; the rest of the math
  surface (`pow`/binary math over arrays, and `log2`/`log10`/`asin`/`acos`/
  `atan`/`floor`/`ceil`/`round`/`trunc` on arrays — trivial follow-ups on the
  Increment 7 kernel). (Shipped: dtype matrix Increment 5 / D-0166; constructors
  Increment 6 / D-0167; the core elementwise math set Increment 7 / D-0168;
  division + scalar-on-left Increment 8 / D-0169; reductions + single `axis=`
  Increment 9 / D-0170; strided N-D views Increment 10 / D-0171; matrix multiply
  `@` / `dot` Increment 11 / D-0172.)
- `uint8` is currently *only* an `array[T]` dtype, not a general scalar type.

## Open issues

- **Views vs copies** for slices: settled (D-0030, #400) — an array slice is a
  view, a list slice is a copy. Both contiguous and strided (stepped / reversed /
  N-D) array views now ship (Increment 10, #502 / D-0171).
- Whether `@simd`/alignment hints on loops are exposed to users (later).
- Integer-array overflow: follows D-0005 (wrap) unless a checked mode is added.
