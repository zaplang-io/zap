---
title: A bigint is rejected in a fixed-width-numeric Optional payload / struct field
status: Locked
issue: 1379
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# A bigint is rejected in a fixed-width-numeric Optional payload / struct field

**Status:** Locked

## Decision

A `bigint` value flowing into a fixed-width numeric (`int*` / `float*`) slot is
rejected with `ZAP4001`, in every position — not only the direct binding. A
`bigint` is never implicitly assignable to a fixed-width numeric: sema already
rejects `int64 x = <bigint>` and `float64 f = <bigint>`. Lowering now rejects the
same source in the positions sema does not type-check — an `Optional[T]` payload
(`Optional.Some(<bigint>)`, a bare-payload reassignment / call argument / field
default, `Optional[int64] x = <bigint>`) and a struct-constructor field — rather
than letting `coerce` silently store the `bigint` heap pointer into the scalar
slot.

The consistent behaviour is **reject**, not a checked narrowing: `bigint` is
rejected in *both* the `int64` and `float64` direct bindings, so an implicit
`bigint -> fixed-width` conversion exists nowhere in the language; admitting it
only in `Optional`/field positions would be the inconsistency. (A consequence,
unchanged here: `int64::MIN` is not spellable as a literal, because its magnitude
`9223372036854775808` overflows `int64` and is a `bigint`; that is the same in
every position, and whether to make it spellable is a separate question.)

## Rationale

`Optional.Some(-9223372036854775808)` stored a `bigint` pointer directly into the
`Optional[int64]` payload's `int64` field; a later `.is_some()` / `.unwrap()`
read the pointer bits as the value (garbage) or, for a tagged `Optional`, read it
as `None` and raised — a memory-unsafe miscompile in a checked program. The root
is a pair of gaps: sema's #1265 bare-payload -> `Optional[T]` widening is
**payload-agnostic** (sema erases the `Optional` payload, tracking it as opaque
`Named("Optional")`, and defers the payload check "downstream"), struct fields are
not sema-type-checked at all, and the "downstream" `coerce` had **no
`bigint -> int*` arm**, so it fell through to its final `return v` no-op.

## Placement

The rejection lives in IR lowering, at the shared `expr_coerced` chokepoint (which
every constructor argument/field, call argument, return and typed binding flows
through) and the bare-payload `Optional`-wrap arm of `expr_collection_context`
(plain reassignment), via one `check_numeric_payload(from, to)` helper. It is in
lowering rather than sema because sema has erased the `Optional` payload type by
the time it checks assignability (`coercible`), so it cannot see that the payload
is `int64`; lowering has the concrete payload type. A consequence is that this
specific error surfaces at `zap build` / `zap ir`, not through the sema-only MCP
`check_source` surface; moving it earlier would require threading the `Optional`
payload type through sema (the broader #1265 "verify the payload" follow-up).

## Consequences

- No new ZIR `Inst` or runtime call; the rejected programs simply fail to lower
  with a clear `ZAP4001` instead of miscompiling. Accepted programs are
  byte-identical: a genuine `Optional[bigint]`, and any in-range integer payload,
  are unaffected.
- Known related facet, not addressed here: `Optional.Some(bigintVariable)` into an
  `Optional[int64]`-typed binding monomorphizes `Optional$bigint` and is then
  conflated with `Optional[int64]` through the same sema payload-erasure (both
  are `Named("Optional")`); isolating that needs payload-aware `Optional`-to-
  `Optional` assignability and is tracked separately.

<!-- Fix in crates/ir/src/lower.rs (`check_numeric_payload`, called from
     `expr_coerced` and the `expr_collection_context` Optional-wrap arm). The
     over-accepting sema rule is the #1265 widening at coercible(). -->
