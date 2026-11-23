---
title: Variadic min/max and element-typed sum
status: Locked
issue: 1284
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# Variadic `min`/`max` and element-typed `sum`

**Status:** Locked

## Decision

Extend the built-in reduction functions (epic #1286), still keeping integers
exact:

- **Variadic `min(a, b, c, …)` / `max(a, b, c, …)`** — two or more numeric
  scalar arguments. The result is the numeric join of all operands; the lowering
  folds a compare + `select` left to right, so integer operands stay in the
  integer domain (an `icmp`, never through `f64`), and a tie keeps the current
  accumulator — i.e. ties resolve to the **first** argument, matching Python and
  the two-arg form (#1279). This generalizes the two-arg case (`n == 2` emits the
  identical code as before).
- **Element-typed `sum(iterable)`** — `sum` is now typed as the iterable's
  element type rather than loosely (`Unknown`). `sum(list[int64])` is an exact
  `int64` that binds into an int slot and never widens to float; a float-element
  iterable is `float64`. An `array[T]` and any iterable whose element type is not
  a known numeric scalar stay loose, where the existing lowering computes the
  real scalar. The runtime reduction was already element-typed (it accumulates
  with a per-type `+`); only the static type changes, so exactness carries from
  run time into the type system.

### Still rejected / deferred

- `min`/`max` keep rejecting a single argument (ZAP4025 — the iterable
  `min(xs)` form is a follow-up), any keyword argument (ZAP4023 — `key=` and
  `default=` are follow-ups), a non-numeric or non-scalar operand (ZAP4024), and
  mixing a signed with an unsigned operand across any arity (ZAP4015).
- The **iterable** `min(xs)`/`max(xs)` reduction, `key=`, and `default=` are a
  follow-up: they need a reduction-loop lowering with empty-sequence handling and
  a function-value projection, which warrants its own focused change. `sum`'s
  optional `start=` is likewise deferred.

## Rationale

- **Variadic is a pure, exact extension.** Folding the existing two-arg compare +
  `select` keeps the whole call in the integer domain for integer operands, so
  the #1279 exactness guarantee holds for `min(a, b, c, …)` with no new
  machinery; it is fully verifiable without a runtime loop.
- **Typing `sum` closes an exactness gap in the type system.** The reduction
  already produces an exact integer at run time; leaving the static type `Unknown`
  let `sum(list[int64])` silently satisfy a `float64` (or any) slot. Typing it as
  the element type makes `sum` usable in precise contexts and surfaces real
  mismatches, while arrays and loosely-typed iterables keep their prior behavior.
- **Deferring the iterable/key= forms keeps the slice tight and testable.** Those
  introduce control flow (empty → `ValueError` / `default=`) and a function-value
  call in a loop whose correctness only the full codegen build exercises; they
  belong in a follow-up rather than enlarging this change.

## Consequences

- `min(a, b, c, …)` and `max(a, b, c, …)` work for any arity ≥ 2 and stay exact
  for integers (golden `scalar_minmax_variadic`).
- `sum(list[int64])` is statically `int64` and exact across the full range
  (including values above 2^53); `sum(list[float64])` is `float64`. Code that
  previously relied on `sum`'s loose `Unknown` to satisfy a *narrower-than-element*
  slot would now see a type error — a deliberate tightening.
- Part of epic #1286. Follow-ups: iterable `min(xs)`/`max(xs)` + `key=` +
  `default=`, `sum(iterable, start=…)`, `str`/`bigint`/`complex` support, and the
  idiomatic-loop built-ins (#1285).
