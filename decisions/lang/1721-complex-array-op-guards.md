---
title: Complex array comparisons and reductions are rejected at lowering until kernels exist
status: Accepted
issue: 1721
pr:
date: 2026-11-28
domain: lang
supersedes:
superseded-by:
---

# Complex array comparisons and reductions are rejected at lowering until kernels exist

**Status:** Accepted

## Decision

On an `array[complex]`, the operations that previously fell through to an int/float
kernel are now rejected at lowering, mirroring #1719 (complex `@`/`dot`) and the
existing sort/argsort/unique complex guards:

- **Comparisons** `< <= > >=` — rejected as "complex arrays have no ordering",
  matching Zap's existing decision that complex has no ordering (`sort`/`argsort`
  already reject it). Permanent.
- **Comparisons** `== != ` — rejected as "not yet supported" (meaningful for complex;
  the kernel is deferred to #938).
- **Reductions** `min` / `max` (`a.min()`/`a.max()`, full or `axis=`) — rejected as "no
  ordering" (permanent, same rationale as `sort`).
- **Reductions** `prod` / `mean` (full or `axis=`) and `sum(axis=)` — rejected as "not
  yet supported" (meaningful; kernels deferred to #938).
- Full `sum(a)` / `a.sum()` over a complex array **stays supported**
  (`zap_array_sum_complex` exists and is correct).

No complex kernels are added here; this is the stopgap that stops the mis-route.

## Rationale

A `complex` array element (#938) is a 16-byte `(re, im)` f64 pair carried at runtime as
a pointer-word. These ops dispatch on `is_float()` / dtype and had no `DTYPE_COMPLEX`
case:

- `lower_array_cmp` → `zap_array_cmp` / `zap_array_cmp_scalar` compare via
  `cmp_domain` + `mm_load_{f,u,i}`; a complex operand resolves to the integer domain and
  the 16-byte complex word is read as a raw `int64` → **silent garbage** for every
  predicate, `==` included.
- `lower_array_method` reductions: `min`/`max`/`prod`/`mean` emit
  `zap_array_{min,max,prod,mean}_complex`, none of which is defined (only
  `zap_array_sum_complex` exists) → **link error**; `sum(axis=)` / `mean(axis=)` route to
  `zap_array_reduce_axis` / `zap_array_mean_axis`, whose dtype switch has
  `default: break` and no complex case → the result buffer is allocated but **never
  filled** (silently wrong).

Found by the AGENTS.md "scan for similar problems" rule while fixing #1719; matmul was
the first op of this family caught, these are the rest. The rejection is a compile-time
`LowerError` (consistent with #1719 and the sort/unique guards) — the safest option and
the one testable without an LLVM toolchain (local LLVM-18 is unavailable; CI is the sole
end-to-end gate). The "no ordering" cases are permanent (Zap chose complex has no
ordering); the "not yet supported" cases name #938 so a future kernel removes the guard
and turns them into value computations.

## Consequences

- Complex array comparisons / `min`/`max`/`prod`/`mean` / axis-`sum` are clean compile
  errors instead of silent corruption or a link error. This is a **behavior change** on
  a pre-1.0, Experimental surface (complex arrays, #938); §3.2/§3.3 of `STABILITY.md`
  already license it, so no policy amendment is made. Plain `err()` (no new `ZAPnnnn`
  code), matching the sibling guards.
- Only two lowering functions change (`lower_array_cmp`, `lower_array_method`); the
  builtin `min(xs)`/`max(xs)` already require a `list` (arrays error there) and there is
  no `prod`/`mean` builtin, so no other entry point can reach a complex reduction.
- Real complex comparison / reduction kernels (`==`/`!=`, `prod`, `mean`, and the axis
  forms) remain tracked under the complex-array epic #938. Full `sum` is unchanged.

<!-- Related: lang/1719-complex-matmul-reject.md (complex @ / dot guard, the sibling this followed from). -->
