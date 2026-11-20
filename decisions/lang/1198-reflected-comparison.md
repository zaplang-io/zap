---
title: Reflected comparison operators — __req__ / __rcmp__ on the right operand
status: Accepted
issue: 1198
pr:
date: 2026-11-20
domain: lang
supersedes:
superseded-by:
---

# Reflected comparison operators — `__req__` / `__rcmp__` on the right operand

**Status:** Accepted

Follow-up of the reflected-operators epic I#1189; extends
[lang/1180-reflected-operators](1180-reflected-operators.md) (which shipped the
arithmetic/bitwise/shift/matmul reflected set and explicitly deferred
comparison) to the comparison operators. Amends
[`design/0006-types-and-generics.md`](../../design/0006-types-and-generics.md)
§2.5.

## Decision

Equality and relational operators gain a **reflected** form, dispatched exactly
like the arithmetic reflected operators (left wins by name; the right operand's
reflected dunder is a fallback consulted only on paths that errored before):

- **`==` / `!=`** → `def __req__(self, L other) -> bool` on the right operand.
  Equality is symmetric, so `a == b` lowers to `b.__req__(a)` with no orientation
  concern; `!=` derives by negation.
- **`< <= > >=`** → `def __rcmp__(self, L other) -> int` on the right operand.
  `__rcmp__` returns the sign of `left <=> self` (negative when the **left**
  operand orders before `self`), so `a < b` is `b.__rcmp__(a) < 0` and the four
  relationals derive **identically** to the forward single-method `__cmp__`
  (D-0047) — no sign flip at the use site.

`L` is the left operand's type; conformance is implicit/structural. For `a OP b`:
the left type's forward `__eq__`/`__cmp__` **by name** wins; otherwise, if the
right type defines `__req__`/`__rcmp__` whose parameter accepts the left operand,
that is used; otherwise the existing error (`ZAP4071` naming `__eq__`/`__cmp__`).
This covers both a struct left without the forward dunder and a built-in left
(e.g. `3 < userValue`). Result is always `bool`.

## Rationale

- **Single-method orientation mirrors the forward design.** Zap already derives
  the four relationals from one `__cmp__` and `!=` from `__eq__` (D-0047). Making
  `__rcmp__` return `sign(left <=> self)` keeps the lowering derivation bit-for-bit
  identical to the forward path (`ICmp pred (cmp, 0)`), so there is no per-operator
  reflected dunder and no sign-flip logic to get wrong. Rejected: per-operator
  reflected dunders (`__rlt__`/`__rgt__`/…), which would multiply surface and
  re-introduce the sign bookkeeping the single-method design removed.
- **Consistent with #1180.** Same precedence (left-by-name wins), same
  implicit/structural conformance, same additivity (only error paths are
  occupied), same parameter-acceptance check on the reflected side. A reader who
  knows reflected arithmetic already knows reflected comparison.
- **Equality stays symmetric.** `__req__` needs no orientation rule; `a == b` ==
  `b.__req__(a)`.

## Consequences

- A right-hand user type can be compared against a core/foreign left operand it
  does not own (`coreValue < userType`, `coreValue == userType`).
- No new diagnostic: a missing handler on both sides is still `ZAP4071`
  (`__eq__`/`__cmp__`). Built-in comparisons and left-defined `__eq__`/`__cmp__`
  are unchanged; the change is purely additive.
- `@derive(Eq, Ord)` is unaffected — it generates the forward `__eq__`/`__cmp__`;
  a type wanting the reflected direction writes `__req__`/`__rcmp__` explicitly.
- Still deferred (tracked under epic #1189): the named-bound capability surface
  (I#1199) and full parameter-aware forward/reflected fall-through (I#1200).
