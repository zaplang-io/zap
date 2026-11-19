---
title: stdlib fractions — exact rational Fraction, software over bigint
status: Locked
issue: 982
pr:
date: 2026-11-19
domain: stdlib
supersedes:
superseded-by:
---

# stdlib `fractions` — exact rational `Fraction`, software over `bigint`

**Status:** Locked

## Decision

Add a `fractions` standard-library module exposing an exact rational `Fraction`
value — the natural companion to the exact base-10
[`decimal`](981-decimal-exact-base10.md). A `Fraction` is a numerator and a
denominator, both `bigint`, kept **always in lowest terms** (divided through by
`gcd`) with a **normalized sign** (the denominator is always `> 0`; the sign
rides the numerator; zero is `0/1`). All arithmetic is **pure prelude Zap over
`bigint`**, with **no runtime C**. The surface is Python-faithful: `import
fractions` then `fractions.Fraction(3, 4)`, with the type written bare as
`Fraction` in annotations (the `decimal.Decimal` / `argparse.ArgumentParser` /
`http.Client` convention — module-qualified constructor, bare struct type).
`Fraction` defines the dunder operators, so `a + b`, `a * b`, comparisons and
`str()` work exactly as in Python.

- **`+ - * /` are always exact** — the result is reduced to lowest terms, so
  there is no rounding and no precision argument (unlike `decimal`'s `/`). The
  exactness headline is `Fraction(1, 3) + Fraction(1, 3) + Fraction(1, 3) ==
  Fraction(1)`, which binary `float64` cannot give.
- **Construction** is from two `int`s (`Fraction(num, den)`; `den == 0` raises
  `ValueError`), from one `int` (`Fraction(7)`), or from a `str`
  (`Fraction("3/4")` ratio form, or a decimal/scientific literal
  `Fraction("0.25")` / `Fraction("1.5e2")`; malformed → `ValueError`).
- **A `float` argument is a compile error** (`ZAP4024`, steering to the `str`
  form), mirroring `decimal`: a silent float → Fraction hop would reinstate the
  binary-rounding hazard the exact type removes. Construct an exact decimal value
  from its `str` (`Fraction("0.1")` is exactly `1/10`).
- **Accessors** `numerator()` / `denominator()` return the reduced `bigint`
  components. **`limit_denominator(max_denominator=1000000)`** returns the
  closest fraction with a denominator at most the bound (CPython's
  continued-fraction-convergent algorithm). **`to_float()`** converts to
  `float64`; **`str()`** renders `num/den` (or just `num` when the denominator is
  `1`), Python-identically.

## Rationale

**Software over `bigint`, not a C library.** Identical to the `decimal` call: the
C-FFI epic (#539) is open and not built, so linking a C rational library would be
premature and the opposite of the language-first default. Software-over-`bigint`
keeps the type fully in-language and dogfoods the existing bignum. A `Fraction`
is a thin pair of `bigint`s normalized through one `gcd` — there is nothing a C
library buys here. Rejected: a C rational library.

**A module with a `Fraction` object, not a keyword builtin.** `bigint`/`complex`
are keyword types, but Python's own API is a `fractions` *module* with a
`Fraction` *class*, and the workspace convention is to match the popular
library's surface (the `decimal` precedent). Operator ergonomics are preserved
because `Fraction` carries the dunder operators (operator overloading was
`D-0047`). Rejected: a keyword `rational` type.

**Always reduced, always sign-normalized.** Storing the canonical form after
every operation makes equality structural (`Fraction(2, 4) == Fraction(1, 2)` is
`num`/`den` equality, no cross-multiplication needed), keeps the `bigint`s as
small as the value allows, and matches Python's invariant exactly. The one `gcd`
per constructor is cheap against the `bigint` multiplies the arithmetic already
does. Rejected: lazy/unreduced storage (would make `==` and `str()` ambiguous and
leak denominator blow-up).

**Exact `/`, no precision argument.** Unlike `decimal`, rational division is
*always* exact — `a/b ÷ c/d = ad/bc`, reduced — so there is nothing to round and
no per-call precision or rounding mode. Division by a zero `Fraction` drives the
denominator to `0` and raises `ValueError` through the same normalizer that
guards `Fraction(n, 0)`. This is the whole point of the rational type and the
clean split from `decimal`.

**Reject float construction.** Same reasoning as `decimal`: binary `float64`
cannot represent most base-10 values, so `Fraction(0.1)` would *not* be `1/10`.
Rather than silently adopt the exact binary expansion (what CPython's
`Fraction(0.1)` does — `3602879701896397/36028797018963968`), Zap makes a float
argument a compile error with a greppable steer to the `str` form, so the author
writes the value they mean (`Fraction("0.1")` → `1/10`). Rejected: an implicit
float constructor (and `from_float`, deferred — a decimal `str` covers the need).

## Consequences

- **Divergences from Python** (also recorded in `docs/PYTHON_DIFFERENCES.md`):
  no implicit `int → Fraction` promotion for operators (Python's `Fraction(1,2) +
  1` works; Zap requires `Fraction(1, 2) + Fraction(1)` — the `decimal` model,
  dictated by Zap's type-first operator dispatch, which dispatches on the left
  operand and does not coerce the right); construction from a `float` is a
  compile error (CPython allows it, exact-binary); `Fraction("1/-2")` with a
  **signed denominator** raises (CPython allows a sign only on the numerator, so
  Zap matches that and rejects it); `numerator()`/`denominator()` are **methods**
  returning `bigint`, not Python `int` properties (Zap has no properties; the
  values are arbitrary-precision); no `from_float` / `from_decimal`
  classmethods (a decimal `str` is the exact path); `ZeroDivisionError` →
  `ValueError` (Zap's flat exception model, the `statistics`/`decimal`
  precedent).
- **No `Fraction` literal.** There is no rational literal syntax; construct via
  `fractions.Fraction(...)`. (No grammar change in this cut.)
- **Seam.** The constructor is a single row in `crates/syntax/src/seam_sigs.rs`
  for the keyword form: `("fractions", "Fraction", &["numerator",
  "denominator"])`, where `denominator` is the **trailing optional** dropped by
  the single-argument `Fraction(int)` / `Fraction(str)` forms (so
  `Fraction(numerator=3, denominator=4)` and `Fraction(numerator="3/4")` both
  bind by name). Everything else — `+ - * /`, comparisons, `numerator`,
  `denominator`, `limit_denominator` (with its `max_denominator` default),
  `to_float`, `str()` — is prelude-struct methods whose keyword/default arguments
  ride `method_param_info` (I#481), so they take no seam rows.
- **Implementation.** The `Fraction` reference struct + its `__frac_*` /
  `__fraction_*` helpers ship in `zapc_syntax::parser::PRELUDE_SRC` (injected via
  the `fractions` / `Fraction` needles); sema `crates/sema/src/modules/
  fractions.rs` and ir `crates/ir/src/lower/modules/fractions.rs` wire the
  constructor to the prelude free functions. `__fraction_parse` /
  `__fraction_from_pair` lower as throwing calls so their `ValueError` is
  catchable (the single-`int` path, `__fraction_from_int`, never raises). No
  codegen or runtime-C changes.
- **Tests.** `tests/golden/fractions_module` (exhaustive, every expected value
  cross-checked against CPython's `fractions`), `crates/sema/tests/
  fractions_sema.rs` (surface + float rejection + arity + operators + method
  kwargs), `crates/sema/tests/seam_kwargs.rs` (constructor keyword binding).
  `docs/STABILITY.md` records the surface.

<!-- Companion exact-number type: [stdlib/981-decimal-exact-base10.md](981-decimal-exact-base10.md). -->
