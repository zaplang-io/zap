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

Axis reductions, N-D and strided views, `zeros`/`arange`/`sqrt`,
division/comparison kernels, broadcasting in assignment / partial-index
(whole-row) stores, inner-axis SIMD on the broadcast path, and
list-variable/iterable constructors are future design work. The broader examples
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

**Deferred (clean follow-ups, noted so they are not silently missing):**

- Inner-axis SIMD for the broadcast path; broadcasting in assignment /
  partial-index (whole-row) stores `m[i] = row` (#501 follow-up / #511).
- N-D (axis-0) array slicing `m[lo:hi]` and strided views (#502).
- **Strided** slice views (`a[::2]`, `a[::-1]`). A *contiguous* 1-D slice
  (`a[lo:hi]`, `a[:]`, `a[lo:]`, `a[:hi]`) is now a view (#400, D-0030): the
  slice header shares the parent buffer and carries an element `off`set + length
  (see `ZapArray.off` in `zaprt.c`), so a write through either side aliases.
  Pointing a view's `data` into the buffer's interior is unsafe — the precise
  collector forwards `data` as an object start under evacuation — so `data` stays
  at the base and `off` carries the window. A strided view would need per-axis
  strides the 1-D `ZapArray` has not got, so a stepped array slice is rejected at
  compile time for now (a list slice, a copy, supports any step).
- `zeros`/`arange`/`sqrt` and axis reductions. (The full dtype matrix —
  `f32`/`i32`/`i8`/`i16`/`u16`/`u32`/`u64` — shipped in Increment 5, D-0166.)
- Scalar-on-the-left of a non-commutative op (`scalar - array`).
- `uint8` is currently *only* an `array[T]` dtype, not a general scalar type.

## Open issues

- **Views vs copies** for slices: settled (D-0030, #400) — an array slice is a
  view, a list slice is a copy. Contiguous array views ship now; strided views
  (which need per-axis strides) remain deferred above.
- Whether `@simd`/alignment hints on loops are exposed to users (later).
- Integer-array overflow: follows D-0005 (wrap) unless a checked mode is added.
