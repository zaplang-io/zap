---
title: Elide match arms after an exhaustive set of enum-variant arms
status: Locked
issue: 1675
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# Elide match arms after an exhaustive set of enum-variant arms

**Status:** Locked

## Decision

When a `match` scrutinee is statically an enum, lowering tracks which variants
the unguarded arms seen so far cover. Once that set covers **every** variant, the
match is exhaustive and the remaining arms are unreachable at this
specialization, so they are dropped before their bodies are lowered. This is the
enum analog of the single-irrefutable-arm elision of #1668 and the dual of the
statically-dead arm elision of #1326; all three are behaviour-preserving
reachability pruning in `lower_match`.

```zap
def pick[T](T value) -> int64:
    match value:
        case Shape.Circle: return 1
        case Shape.Square: return 2
        case _:                       # dead for T = Shape; not lowered
            fn(int64) -> int64 f = value
            return f(0)
```

For `pick$Shape`, `Circle | Square` is exhaustive, so the `case _:` arm is
unreachable and its body (type-correct only for a callable specialization) never
reaches `check_fn_coercion`, where it previously failed with `ZAP4001: value of
type enum#0 is not a function`.

## What counts toward coverage

Only an **unguarded** arm whose pattern is a clean full-variant cover:

- a bare variant pattern `E.V` (a tier-1 member, or a payload-free tier-3
  variant);
- a variant pattern whose every payload sub-pattern is a bare binder — `E.V(x)` /
  `E.V(_)` — which matches every value of that variant;
- an or-pattern covering the union of its alternatives; and
- `<pat> as name` covering what its inner pattern does.

A **guarded** arm (it may fall through), a variant pattern with a **refutable**
payload (`E.V(0)`, a nested destructure), a literal, and a pattern of some other
enum do **not** count, so coverage is never over-claimed and a non-exhaustive
match keeps its trailing arms live.

Both tier-1 enums (`Ty::Enum`, members) and tier-3 tagged enums (`Ty::Struct`
whose id matches a tagged enum, variants) are covered; the niche-optimized
`Optional`-shaped enum is a two-variant tier-3 enum and exhausts on
`Some(x)` + `None`.

## Rationale

- **Reachability, not exhaustiveness policy.** sema still owns match
  exhaustiveness diagnostics; this is purely dead-code elimination so a body
  reachable only in a *different* specialization is not lowered. An arm after an
  exhaustive variant set never runs, so dropping it cannot change any program's
  output.
- **Conservative coverage.** Only certain full covers count; anything
  runtime-decidable keeps later arms live, so the pruning never removes a
  reachable arm.
- **Lowering, not sema.** The predicate needs the specialized scrutinee type and
  the enum's variant list, both available at IR lowering (after
  monomorphization), beside the #1668/#1326 elisions.

## Consequences

- `crates/ir/src/lower/enums.rs`: `lower_match` accumulates covered variants via
  the new `match_enum_variants` (scrutinee → enum name + variant set) and the free
  `arm_covered_variants` (an arm's clean cover), and `break`s once the set is
  exhaustive — alongside the #1668 single-arm irrefutable `break`.
- Completes the match-pruning mechanism for the generic type-dispatch pattern
  (#1668/#1669); independent of the callback-shape normalization decision (#1674).

<!-- Related: lang/1668-irrefutable-arm-specialization.md (the struct/tuple/
wildcard case), #1326 (statically-dead arms), #1674, #1669. -->
