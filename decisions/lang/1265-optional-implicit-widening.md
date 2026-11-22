---
title: A bare value of type `T` implicitly widens to `Optional[T]` in every position
status: Locked
issue: 1265
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# A bare value of type `T` implicitly widens to `Optional[T]` in every position

**Status:** Locked

## Decision

A concrete value of type `T` is assignable to an `Optional[T]` destination and is
implicitly wrapped as `Some(T)` — in **every** type-contextual position: a
`return`, an `Optional[T]` parameter (call argument), a typed binding, a
struct-constructor field, field/index/key assignment, `.append`, and
collection-literal elements. This matches Python's `x: Optional[int] = 1` and
completes the bare-`None` widening that already held (`None` widens to any
`Optional[T]` everywhere — it infers to `Unknown`, which `assignable` admits).

It resolves the design question deferred by
[`1262`](1262-optional-scalar-coercion.md): **widen everywhere**, not
reject-everywhere. #1262 fixed the memory-safety half (a bare payload stored
unboxed → `.unwrap()` SIGSEGV) by wrapping `T → Some(T)` at the shared IR
lowering chokepoint, but only for the positions sema already accepted a bare
payload in (constructor fields, which sema does not type-check, plus the
assignment/`.append` paths). The remaining positions — `return` (`ZAP4003`), an
`Optional[T]` parameter (`ZAP4021`), a typed binding and a list-literal element
(`ZAP4001`) — rejected a bare payload even though an explicit `Optional.Some(1)`
was accepted. This widens sema to admit them uniformly.

The change is one sema rule: `coercible(from, Optional[T])` is true for any
non-`Optional` `from` (an `Optional` source flows by the ordinary
`Optional`-to-`Optional` identity, never re-wrapped into `Some(Some(..))`). No
IR change is needed — lowering's `expr_coerced` / `expr_collection_context`
chokepoint already wraps every admitted position via `gen_opt_some` (#1262).

## Rationale

`Some` wrapping is unambiguous and total (any `T` has exactly one `Some(T)`), so
the widening never hides a real mismatch between two reference types the way an
implicit numeric or interface coercion might. Accepting it everywhere removes a
surprising asymmetry — `Delta(day=1)` and `return Optional.Some(1)` worked but
`return 1` did not — and matches the mental model every Python/typing user
already has. Reject-everywhere was the alternative; it would have broken the
`Delta(day=1)` ergonomics #1262's datetime-utils use case depends on, for no
soundness gain.

## Consequences

- Previously-rejected programs now compile: `return 1`, `take(1)` into an
  `Optional[int64]` parameter, `Optional[int64] x = 1`, and
  `list[Optional[int64]] xs = [1, 2]` all type-check and lower to a tagged
  `Some`.
- The rule lives in `coercible`, not `assignable`: `assignable` stays the strict
  structural relation (used by invariant positions such as `dict`/`set` keys and
  channels), and the `Optional` widening is a *coercion*, consistent with where
  the struct→interface and collection-covariance coercions already live.

## Payload typing stays downstream (pre-existing, unchanged)

Sema tracks `Optional[T]` opaquely as `Ty::Named("Optional")` with the payload
**erased** (`resolve_type` / `is_optional_ty`), so it cannot and does not verify
that a payload's *own* type matches `T`. This was already true before this
change: `return Optional.Some("x")` for `-> Optional[int64]`, and a mismatched
struct-constructor field, are both accepted by sema today, with payload
correctness enforced downstream (mono / lowering). The widening is therefore
deliberately **payload-agnostic**, exactly matching the existing `Optional.Some`
and constructor-field behaviour; it introduces no new soundness obligation.
Making sema payload-aware for `Optional[T]` (so a genuine payload mismatch is a
clean sema error in *all* `Optional` positions, explicit `Some` included) is a
larger, separate piece of work and is out of scope here.

<!-- The sema rule is the `is_optional_ty(to) && !is_optional_ty(from)` arm in
     `coercible` (crates/sema/src/lib.rs). The IR wrap it relies on is #1262
     (crates/ir/src/lower.rs `expr_coerced` / `gen_opt_some`). -->
