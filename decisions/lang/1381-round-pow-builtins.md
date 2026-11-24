---
title: Python built-in round() and pow() free functions
status: Locked
issue: 1381
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Built-in `round(x[, ndigits])` / `pow(base, exp[, mod])`

**Status:** Locked

## Decision

Add two Python built-in free functions, the numeric follow-ups recorded as
out-of-scope by epic #1286 (`decisions/lang/1279-scalar-min-max.md`,
`decisions/lang/1283-scalar-abs-divmod.md`):

- **`round(x[, ndigits])`** — Python's built-in rounding, which uses **banker's
  rounding (ties-to-even)**, distinct from the float-domain `math.round`
  (half-away-from-zero). Ties-to-even is `rint` under the default
  `FE_TONEAREST` mode.
  - `round(float)` → **`int64`**. `round(nan)`, `round(+/-inf)`, and a result
    outside the `int64` range raise a catchable **ArithmeticError** (Python
    raises ValueError/OverflowError, both ArithmeticError-family; Zap returns a
    fixed-width `int64`, not a bigint).
  - `round(float, ndigits)` → **`float64`**, rounded to `ndigits` decimal places
    (negative `ndigits` rounds to a power of ten). Computed by scaling, `rint`,
    and unscaling — multiplying by `10**ndigits` for `ndigits >= 0`, dividing by
    the exact integer `10**(-ndigits)` for `ndigits < 0` so a clean power-of-ten
    result (`round(12345.0, -2) == 12300.0`) is exact. This is **binary-float**
    scaling for the positive-`ndigits` case, so a
    value whose exact decimal is not representable in `f64` can round like its
    nearest `f64` rather than its decimal literal (the standard float caveat;
    CPython's decimal-correct `dtoa` path is explicitly **not** reproduced).
  - `round(int)` → the integer **unchanged** (preserving signedness:
    `int*`→`int64`, `uint*`→`uint64`).
  - `round(int, ndigits)` → **`int64`**: a no-op for `ndigits >= 0`; for
    `ndigits < 0`, round to the nearest multiple of `10**(-ndigits)` with
    ties-to-even, using integer arithmetic (never through float). The one
    unrepresentable case (a result past the `int64` range) raises
    ArithmeticError rather than wrapping.
- **`pow(base, exp[, mod])`**.
  - **Two arguments** is defined to be exactly **`base ** exp`**: it lowers to
    the same `**` path and therefore reuses the whole numeric tower — int-exact
    `zap_ipow`/`zap_upow`, float `zap_powf`, `zap_bigint_pow`,
    `zap_complex_pow`. A negative integer exponent raises the same catchable
    **ValueError** `**` raises (int stays int; use a float base for a real
    negative power — D-0019, #374/B2). Result type is the numeric join of the
    operands.
  - **Three arguments** is integer **modular exponentiation** `(base ** exp) %
    mod` → **`int64`**, via `zap_imodpow`. Integer operands only
    (float/bigint/complex modular pow are follow-ups). `mod == 0` and a negative
    `exp` each raise a catchable **ValueError** (the modular-inverse case for a
    negative exponent, CPython 3.8+, is a follow-up). The result takes the sign
    of the modulus, matching Python and Zap's floored `%`. The intermediate
    product is kept in `__int128`, so no step overflows.

### Typing

- `round`: see the four cases above. `ndigits` must be an integer (ZAP4024
  otherwise). bigint/complex first arguments are rejected (ZAP4024), matching
  abs/divmod.
- `pow`: two-arg result is `num_join(base, exp)` lifted to complex/bigint/float
  as `**` would; three-arg result is `int64`.
- Both reject keyword arguments (ZAP4023) and wrong arity (ZAP4025). Positional
  only — `seam_sigs.rs` is not touched.

## Rationale

- **Built-in `round` is banker's, `math.round` is not.** Python programmers
  expect `round(0.5) == 0` and `round(2.5) == 2`. Reusing `math.round`
  (half-away-from-zero) would silently disagree, so the built-in gets its own
  `rint`-based helpers and keeps `math.round` unchanged.
- **`pow(b, e)` *is* `**`.** Rather than re-deriving the numeric tower, the
  two-argument form delegates to the existing `**` lowering, so it is correct by
  construction (int-exact, bigint, complex) and stays consistent with the
  operator — including the negative-integer-exponent ValueError.
- **Integer exactness.** Both the `round(int, …)` path and modular `pow` stay in
  the integer domain; `zap_imodpow` uses `__int128` intermediates so a 63-bit
  modulus never overflows the running product. This is the same exactness
  discipline as #1279/#1283.
- **Raise, don't wrap or silently misround.** Out-of-range `round` results and a
  zero / negative modular argument raise catchable errors (the `zap_panic_typed`
  mechanism abs/divmod already use), never a wrapped or silently-wrong value.
- **Binary-float `round(float, ndigits)` caveat is accepted, documented.**
  Decimal-correct rounding needs CPython's `dtoa`; the scale/`rint`/unscale
  approach matches CPython for the overwhelming majority of inputs and is what
  most compiled languages ship. Recorded here so it is a known, intentional
  divergence, not a latent bug.

## Consequences

- `round` and `pow` now shadow user free functions of those names, like the
  other built-ins.
- `pow`'s three-argument modular form and `round`'s float+ndigits path are new
  runtime helpers (`zap_imodpow`, `zap_round_i64`, `zap_round_nd`,
  `zap_iround_nd`); their behavior is exercised by golden `scalar_round_pow` and
  gated by the LLVM-18 CI job.
- Follow-ups (not in this change): `bigint`/`complex` modular `pow`, the
  negative-exponent modular inverse (CPython 3.8+), and decimal-correct
  `round(float, ndigits)`.
