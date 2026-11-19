---
title: stdlib decimal — exact base-10 Decimal, software over bigint
status: Locked
issue: 981
pr: 1093
date: 2026-11-19
domain: stdlib
supersedes:
superseded-by:
---

# stdlib `decimal` — exact base-10 `Decimal`, software over `bigint`

**Status:** Locked

## Decision

Add a `decimal` standard-library module exposing an exact base-10 `Decimal`
value for money / billing. A `Decimal` is a coefficient (a `bigint`, which
carries the sign) plus an `int64` exponent — value = `coeff × 10^exp` — and all
arithmetic is **pure prelude Zap over `bigint`**, with **no runtime C**. The
surface is Python-faithful: `import decimal` then `decimal.Decimal("1.50")` /
`decimal.Decimal(42)`, with the type written bare as `Decimal` in annotations
(the `argparse.ArgumentParser` / `http.Client` convention — module-qualified
constructor, bare struct type). `Decimal` defines the dunder operators, so
`d1 + d2`, `d1 * d2`, comparisons and `str()` work exactly as in Python.

- **`+ - *` are always exact** (the result scale is deterministic: add/sub → the
  finer input scale, mul → the sum of scales). This exactness is the correctness
  guarantee — `Decimal("0.1") + Decimal("0.2") == Decimal("0.3")`.
- **Division is per-operation, with no global context.** `a.div(b, prec=28,
  rounding="ROUND_HALF_EVEN")`; the `/` operator uses those same per-call
  defaults. Exact quotients reduce to the ideal exponent (`e1 − e2`), matching
  the General Decimal Arithmetic spec / CPython (`Decimal(1)/Decimal(2)` → `0.5`).
- **Rounding** is a string tag, default banker's `"ROUND_HALF_EVEN"`, with the
  full eight-mode GDAS set: `ROUND_HALF_EVEN`, `ROUND_HALF_UP`, `ROUND_HALF_DOWN`,
  `ROUND_DOWN` (truncate toward zero), `ROUND_UP` (away from zero), `ROUND_FLOOR`
  (toward −∞), `ROUND_CEILING` (toward +∞), `ROUND_05UP`. `quantize(exemplar,
  rounding=…)` sets the scale; an unknown tag raises `ValueError`.
- **Construction is never lossy from a float.** `decimal.Decimal(<float>)` is a
  compile error (`ZAP4024`, steering to `from_float`); construction is from a
  `str` (parsed; malformed → `ValueError`) or an `int` (exact). The explicit
  `decimal.from_float(f)` opt-in converts through the float's shortest
  round-trip decimal string.
- Also `to_int` (truncate toward zero), `to_integral_value`, `exponent`,
  `is_zero`, value comparisons (scale-independent, so `1.0 == 1.00`), and
  `str()` / parsing per the GDAS to-scientific-string rules.

## Rationale

**Software over `bigint`, not a C library.** The C-FFI epic (#539) is open and
not built, so linking libmpdec would be premature; and it is the opposite of the
language-first default. Software-over-`bigint` keeps the type fully in-language
and dogfoods the existing bignum. Rejected: linking a C decimal library.

**A module with a `Decimal` object, not a keyword builtin.** `bigint`/`complex`
are keyword types, but Python's own API is a `decimal` *module* with a `Decimal`
*class*, and the workspace convention is to match the popular library's surface.
Operator ergonomics are preserved because `Decimal` carries the dunder operators
(see [tooling/.. operator overloading was `D-0047`]). Rejected: a keyword
`decimal` type (diverges from Python's module+class).

**Explicit per-operation precision, no global context.** `+ - *` are exact;
only division can be non-terminating, so it takes an explicit precision +
rounding per call. A Python-style thread-local/global context is hidden mutable
state — exactly the implicit shared-state hazard a statically-typed,
concurrency-first language avoids. Rejected: a global/scoped mutable context (a
scoped context *object* was also considered and set aside as more verbose than
per-call keywords for v1; it remains a possible later addition).

**Half-even default, full GDAS set as string tags.** Banker's rounding is the
IEEE-754 / financial norm and Python's default. String tags are the
static-language adaptation already used for argparse's `action`/`type`; the tag
values equal Python's `decimal.ROUND_*` constant *values*, so the surface is
near-identical. Rejected: exposing named `ROUND_*` constants in v1 (would need
string-constant lowering; the string default means the common call needs no tag
— a trivial possible follow-up).

**Reject float construction.** Binary `float64` cannot represent most base-10
values, so a silent float → Decimal hop would reinstate exactly the rounding
hazard the type removes. Making it a compile error with an explicit, greppable
`from_float` opt-in keeps the lossful step visible. Rejected: an implicit float
constructor.

## Consequences

- **Divergences from Python** (also recorded in `docs/PYTHON_DIFFERENCES.md`): no
  global/thread-local context; no `Infinity`/`NaN` (finite-only — parsing one
  raises `ValueError`); no signed zero (a zero result is always unsigned);
  Python's `InvalidOperation` / `DivisionByZero` map to the flat-model
  `ValueError` (Zap has no exception subclassing); `quantize` performs no
  context-precision overflow check; `from_float` is shortest-repr, not
  exact-binary.
- **No decimal literal.** A `1.50d`-style literal is a grammar/token change and a
  larger decision; it is not part of this cut — construct via
  `decimal.Decimal("1.50")`. Flagged as a possible later grammar follow-up.
- **Seam.** The two constructors are positional single values, given rows in
  `crates/syntax/src/seam_sigs.rs` for the keyword form: `("decimal", "Decimal",
  &["value"])`, `("decimal", "from_float", &["f"])`. Everything else — `+ - * /`,
  `quantize`, `div`, comparisons, `to_int`, `exponent`, `is_zero` — is
  prelude-struct methods whose keyword/default arguments ride `method_param_info`
  (I#481), so they take no seam rows.
- **Implementation.** The `Decimal` reference struct + its `__dec_*` /
  `__decimal_*` helpers ship in `zapc_syntax::parser::PRELUDE_SRC` (injected via
  the `decimal` / `Decimal` needles); sema `crates/sema/src/modules/decimal.rs`
  and ir `crates/ir/src/lower/modules/decimal.rs` wire the two constructors to
  the prelude free functions. `__decimal_parse` / `from_float` lower as throwing
  calls so their `ValueError` is catchable. No codegen or runtime-C changes.
- **Tests.** `tests/golden/decimal_module` (exhaustive, every expected value
  cross-checked against CPython's `decimal`), `crates/sema/tests/decimal_sema.rs`
  (surface + float rejection), `crates/sema/tests/seam_kwargs.rs` (constructor
  keyword binding). `docs/STABILITY.md` records the surface.
