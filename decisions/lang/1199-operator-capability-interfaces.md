---
title: Operator capabilities are declared prelude generic interfaces (forward + reflected)
status: Accepted
issue: 1199
pr:
date: 2026-11-20
domain: lang
supersedes:
superseded-by:
---

# Operator capabilities are declared prelude generic interfaces (forward + reflected)

**Status:** Accepted (design locked; implementation blocked on generics)

Follow-up of the reflected-operators epic #1189. Records the surface for #1199
(naming a reflected capability as a generic bound). Builds on
[lang/1180-reflected-operators](1180-reflected-operators.md) and
[lang/1198-reflected-comparison](1198-reflected-comparison.md); governing
generics design `design/0012-generics.md`.

## Decision

The operator capabilities — both the **forward** set (`Add`, `Sub`, …, `Mul`,
`Eq`, `Ord`, …) and the **reflected** set (`RAdd`, `RSub`, …, `RMul`, `RReq`-style
equality / `RCmp`-style ordering) — become **declared prelude generic
interfaces**, user-nameable as generic bounds, rather than purely
compiler-known method shapes. The reflected interfaces carry the left-operand
type and an output type, e.g.:

```zap
interface RMul[L, Out]:
    def __rmul__(self, L other) -> Out
```

`def scale_all[T: RMul[f64, T]](xs: list[T], k: f64) -> list[T]` then constrains
`T` to scalar-right-multipliable types and names the result. (`[L, Out]` is the
two-parameter MVP; a later associated-type form `RMul[L]` + `type Out` is #1207.)

This is **chosen over** keeping the capabilities structural with the bound as a
compiler-recognized alias (option B).

## Rationale

- **One mechanism, not two.** Operator overloads are already conceptually
  structural interfaces (D-0047 shows `interface Add`). Declaring them for real
  — uniformly for forward and reflected — avoids a parallel "structural
  recognition here, declared-alias there" split that would have to be kept in
  sync. Option B keeps two mechanisms; A collapses them.
- **Nameable, greppable, documentable.** Generic code can state its operator
  requirements as ordinary bounds, and the capability set is discoverable in the
  prelude instead of implied by compiler internals.
- **Dispatch is unaffected.** Operator *dispatch* (forward and reflected) already
  ships and works via structural recognition (#1192/#1194/#1202); declaring the
  interfaces changes how capabilities are **named and bounded**, not how a
  concrete `a OP b` resolves. The structural fast path remains for non-generic
  code.

Rejected — **(B) keep structural + bound alias**: smaller surface but two
mechanisms to maintain, and no path to naming the output type cleanly.

## Consequences

- **Blocked on generics realization.** Needs generic-interface conformance
  (#1205) and bounds referencing other type params (#1206); the associated-type
  form is optional (#1207). All three sit under the generics epic #1204. #1199
  implementation starts once #1205 + #1206 land.
- When implemented: add the declared forward + reflected operator interfaces to
  the prelude; teach sema to check conformance and resolve the output type;
  enforce parametrized bounds; route bounded-generic operator calls through the
  interface in monomorphization. The non-generic structural dispatch stays.
- No change to today's behavior or spec surface until the prerequisites land;
  `DECISIONS.md` stays frozen (this ADR is the record).
