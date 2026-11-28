---
title: Complex `@` / `dot` is rejected at lowering until a complex kernel exists
status: Accepted
issue: 1719
pr:
date: 2026-11-28
domain: lang
supersedes:
superseded-by:
---

# Complex `@` / `dot` is rejected at lowering until a complex kernel exists

**Status:** Accepted

## Decision

`a @ b` and `dot(a, b)` on a `complex` array (or any complex↔real mix) are rejected
with a lowering error in `lower_array_matmul` — the same kind of rejection
sort/argsort/unique/argmin/argmax already apply to complex — rather than being
dispatched to a kernel. The rejection covers every form: 1-D·1-D dot, 1-D·2-D,
2-D·1-D, 2-D·2-D, both the `@` operator and the `dot()` builtin, and complex@complex /
complex@real / real@complex (a single `promote(…) == Complex` check catches all three,
since `promote` makes either-complex collapse to `Complex`). The dedicated complex
matmul kernel (`zap_matmul_c` / `zap_array_dotc`) is **deferred** to the complex-array
epic #938; this is the size:S stopgap only.

## Rationale

A `complex` array element (#938) is a 16-byte `(re, im)` f64 pair carried at runtime as
a pointer-word. `Dtype::promote(Complex, _) -> Complex` (dtype code 11), and `Complex`
is neither `F32` nor `F64`, so the pre-existing dispatch (`out.is_float()`) sent complex
operands down the **integer** path: 1-D to `zap_array_doti`, 2-D to `zap_array_matmul`'s
`zap_matmul_i`. Those kernels' load/store helpers (`mm_load_i` / `arr_store_i`) have no
`DTYPE_COMPLEX` case, so they read and wrote the complex pointer-word as a raw `int64` —
**silent garbage and memory corruption with no error**. Matmul was the one array op in
this family with no complex guard while its no-ordering siblings (sort/unique/search) all
had one.

**Compile-time rejection, not a runtime raise.** The guard returns a `LowerError`
(compile-time), mirroring the exact mechanism and location of the sort/argsort/unique
guards in the same file — the operation never produces a binary, so the corrupting kernel
can never run. The issue text also described the stopgap as a "catchable `ValueError`"
with a `# raises:` golden; that framing does not fit a `LowerError` (a compile error is not
runtime-catchable, and sort/unique — the ops it points at as the pattern to mirror — are
themselves compile-time rejections, not runtime raises). We chose consistency with the
existing complex guards and the stronger safety of a build-time reject. A future real
complex kernel (#938) removes this guard, at which point these cases become value
computations; the host-only lowering test
(`crates/ir/tests/array_matmul_complex_guard_1719.rs`) is written so that it fails loudly
when the guard is removed, flagging that it must flip to value checks. If the maintainer
prefers a runtime-catchable `ValueError` instead (so today's `try/except` can catch it and
a `# raises:` runtime golden is possible), that is a one-line swap to a `throwing_call`
raising `ValueError` and can be revisited.

## Consequences

- Complex `@` / `dot` is a clean compile error instead of silent corruption. This is a
  **behavior change** for any program that reached the mis-route (it previously
  "compiled and ran" with garbage output); see `STABILITY.md`.
- The complex matmul kernel stays deferred to #938; no runtime kernel is added here.
- Verified siblings in the same "dtype dispatch with no complex case" class — complex
  array **comparisons** (`==`/`!=`/`<`/`<=`/`>`/`>=` via `zap_array_cmp` /
  `zap_array_cmp_scalar`, which `mm_load_i` the complex word) and complex **axis
  reductions** / `mean(axis=)` (`zap_array_reduce_axis` / `zap_array_mean_axis` have no
  `DTYPE_COMPLEX` case, so the result buffer is left unfilled) — are the same silent-wrong
  class and are **reported separately** under #938, not fixed here (unassigned core work).
  The full-reduction `min`/`max`/`prod`/`mean` on complex fail loudly today (undefined
  `zap_array_{min,max,prod,mean}_complex` → link error) and are noted alongside.

<!-- Related: lang/1180-reflected-operators.md (operator dispatch); matmul auto-dispatch D-0181 (#904). -->
