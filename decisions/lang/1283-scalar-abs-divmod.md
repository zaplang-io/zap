---
title: Scalar integer-preserving abs and divmod built-ins
status: Locked
issue: 1283
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# Scalar `abs(x)` / `divmod(a, b)`

**Status:** Locked

## Decision

Add two built-in free functions, Python-compatible in spelling, as the
exactness-twin follow-ups to scalar `min`/`max` (decisions/lang/1279-scalar-min-max.md):

- **`abs(x)`** — the magnitude of one numeric scalar.
  - A signed integer stays an **integer** (`int64`): `|x| = x < 0 ? -x : x`
    lowered with an integer negate and `select`, **never** through `f64`.
  - An **unsigned** value is returned unchanged (`uint64`) — it is already its
    own magnitude.
  - A **float** yields its `float64` magnitude via `zap_math_fabs` (libm
    `fabs`), so `abs(-0.0) == 0.0` and `abs(nan)` is `nan`.
  - `abs(INT64_MIN)` has **no** `int64` result (its magnitude 2^63 overflows),
    so it raises a catchable **ArithmeticError** rather than wrapping back to
    `INT64_MIN`.
- **`divmod(a, b)`** — the `(quotient, remainder)` pair as a 2-tuple of the
  numeric join of the operands (`(int64, int64)`, `(uint64, uint64)`, or
  `(float64, float64)`). It uses the **same floored `//` and `%`** the operators
  do (`zap_ifloordiv`/`zap_imod`, `zap_udiv`/`zap_umod`, `zap_ffloordiv`/
  `zap_ffmod`), so `divmod(a, b) == (a // b, a % b)` for every sign combination,
  and the integer path never goes through float. A zero divisor raises the same
  catchable **ZeroDivisionError** the operators raise.

### Typing

- `abs`: `float32`/`float64`/untyped-float → `float64`; unsigned → `uint64`;
  signed int → `int64`.
- `divmod`: result is `Tuple[T, T]` where `T = num_join(a, b)`.
- Both reject: wrong arity (ZAP4025), any keyword argument (ZAP4023), a
  non-numeric or non-scalar operand including `bigint` and `complex` (ZAP4024);
  `divmod` also rejects mixing a signed with an unsigned operand (ZAP4015), the
  same rule the comparison operators enforce.
- Positional-only — `seam_sigs.rs` is not touched (neither is a seam function).

## Rationale

- **Same exactness hazard as #1279.** Integer `abs` through `math.fabs` (the only
  prior option) round-trips `int64` through `f64` and loses precision above 2^53;
  there was no scalar `divmod` at all. Keeping both in the integer domain is the
  point.
- **Reuse the blessed machinery.** `divmod` emits exactly the floored-division
  and modulo helpers the `//`/`%` operators already use, so quotient/remainder
  are consistent with them by construction, including the floored-sign rule
  (D-0024) and the zero-divisor raise. The 2-tuple is built like any tuple
  literal / `return a, b`.
- **Raise, don't wrap, on INT64_MIN.** Silently returning `INT64_MIN` from
  `abs(INT64_MIN)` is the footgun class Zap avoids; an explicit ArithmeticError
  (Python raises OverflowError, a subclass of ArithmeticError, which Zap has)
  makes the one unrepresentable case loud. The guard is an inline compare +
  `zap_panic_typed`, the same mechanism `@validate` and the div/mod faults use —
  no new runtime surface.
- **`fabs` for floats, not a sign-flip select.** `zap_math_fabs` is already
  linked and declared unconditionally (`modules::declare_runtime`), and it is
  correct for `-0.0` and `NaN`, which a `x < 0 ? -x : x` select is not.

## Consequences

- Integer `abs`/`divmod` are exact across the full range, negatives, and the
  floored-sign quadrants (golden `scalar_abs_divmod`).
- `abs`/`divmod` now shadow user free functions of those names, like the other
  built-ins.
- Part of epic #1286. Still follow-ups there: the variadic/iterable/`key=`/
  `default=` reduction forms and int-exact `sum` (#1284), the idiomatic-loop
  built-ins (#1285), and `bigint`/`complex` support for `abs`/`divmod`.
