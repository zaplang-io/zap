---
title: Reflected binary operators — right-operand fallback dispatch (capability-based)
status: Accepted
issue: 1180
pr:
date: 2026-11-20
domain: lang
supersedes:
superseded-by:
---

# Reflected binary operators — right-operand fallback dispatch (capability-based)

**Status:** Accepted

Governing issue I#1180; epic I#1189 (slices: sema I#1186/#1192, lowering
I#1187/#1194, this spec I#1188). RFC: Discussion #1182. Amends the operator
section of [`design/0006-types-and-generics.md`](../../design/0006-types-and-generics.md)
§2.5 (D-0047, which committed to left-operand-only dispatch).

## Decision

A user-defined type may handle a binary operator as the **right** operand by
defining the matching **reflected dunder**, recognized structurally like every
other operator interface (D-0016/D-0047 — no `impl`, no annotation). For `a OP
b` with forward dunder `D` and reflected dunder `R`, dispatch resolves
**statically** as:

1. **Left wins.** If the built-in rules type the operator, or the left type is a
   struct that defines `D` **by name**, use it (`a.D(b)`) — unchanged from
   D-0047. The RHS is not re-checked against `D`'s parameter: selecting the
   forward dunder by name is the existing rule.
2. **Reflected fallback.** Otherwise, if the right type is a struct whose `R`'s
   first non-`self` parameter accepts the left operand's type, use `b.R(a)`
   (`self` = right operand). The result type is `R`'s declared return type.
3. **Otherwise** `ZAP4071`.

The reflected set mirrors the binary dunder table one-for-one:

| `a OP b` | forward | reflected |
|---|---|---|
| `+ - * / // % **` | `__add__ __sub__ __mul__ __truediv__ __floordiv__ __mod__ __pow__` | `__radd__ __rsub__ __rmul__ __rtruediv__ __rfloordiv__ __rmod__ __rpow__` |
| `& \| ^ << >>` | `__and__ __or__ __xor__ __lshift__ __rshift__` | `__rand__ __ror__ __rxor__ __rlshift__ __rrshift__` |
| `@` | `__matmul__` | `__rmatmul__` |

**Edge — the left defines `D` but its parameter does not accept the RHS:** this
is an ordinary type error, **not** a reflected reroute. A left type that defines
the forward dunder by name commits the operator to the left.

**Not covered:** comparison (`__eq__`/`__cmp__`), membership (`in`, which already
dispatches on the container), indexing, call, unary, and the in-place `__iOP__`
family / augmented assignment — none has a reflected form. `x in a` is unchanged.

## Rationale

- **Implicit/structural, not explicit.** Every operator capability in Zap is
  satisfied structurally today (`Add`, `Ord`, `Eq`, `Hash`); an explicit `impl`
  form would contradict all of them. The orphan/authorization gate an explicit
  form would buy is moot: `R`'s `self` is always the type being defined, so a
  reflected operator can only be added to a type one owns — a foreign type is
  never retroactively modified.
- **Error, not Python-style fall-through, at the "left exists but rejects RHS"
  edge.** Zap is statically typed with no `NotImplemented` runtime retry; the
  fall-through rule would require parameter-type-aware forward resolution, which
  sema cannot provide for generic lefts (generics are erased to `Named`, type
  vars to `Unknown`, and sema has no function-type model — the comparison lives
  only in lowering). The motivating consumer does not need it: core
  `DateTime`/`Date` define no forward `__add__`/`__sub__` at all, so the
  name-level fallback already reaches a package `RelativeDelta.__radd__`.
- **Purely additive.** Reflected only occupies paths that errored before
  (`ZAP4071` for a struct left without the forward dunder; `ZAP4012` for a
  built-in left with no rule), so no previously-compiling program changes
  meaning, and all built-in arithmetic and existing left-hand overrides are
  untouched.
- **Full set specified now** so there is no second naming/grammar RFC; the cost
  over just `__radd__`/`__rsub__` is a static name-mapping table.

### Alternatives rejected

- **Keep left-only (decline).** Would force non-core packages into
  `rd.add_to(dt)` / `datetime.shift(dt, rd)` instead of `dt + rd`, losing
  dateutil parity — the specific thing I#1180 / datetime-utils#2 ask for.
- **Full Pythonic param-aware fall-through (adopt-full).** Needs overload
  resolution moved into lowering; much larger, and no consumer requires the
  `Duration + X`-style case it would add. Left as a possible future epic.
- **Explicit `interface RMul[L, Out]` as a required bound.** The nameable-bound
  form depends on generic interfaces (grammar-reserved, not realized) +
  bounds-referencing-type-params (deferred, `design/0012-generics.md`), none of
  which dispatch needs. Kept as a forward-compatible future surface, not a
  prerequisite.

## Consequences

- A right-hand user type can participate in `+ - * / // % ** & | ^ << >> @` with
  a left operand it does not own — unblocking datetime-utils#2's `dt + rd` /
  `dt - rd` once the implementation slices (#1192, #1194) are in.
- Reflected dunder calls lower through `throwing_call` (invoke), so a raising
  reflected dunder is catchable by `try/except`.
- `design/0006` §2.5's "there is no reflected `__radd__`" statement is replaced
  by the rule above; `DECISIONS.md` (frozen) is not edited — this ADR is the
  going-forward record.
- Future work (not committed here): reflected comparison, the named-bound
  capability surface, and full param-aware fall-through, each gated on real
  demand and the generics slices above.
