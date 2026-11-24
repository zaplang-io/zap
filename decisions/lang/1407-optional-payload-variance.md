---
title: An Optional[U] is not assignable to an Optional[T] with a different payload
status: Locked
issue: 1407
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# An Optional[U] is not assignable to an Optional[T] with a different payload

**Status:** Locked

## Decision

A whole `Optional[U]` value is assignable to an `Optional[T]` slot only when the
payloads are identical (`U == T`). A mismatch is rejected with `ZAP4001`, in every
position — typed binding, reassignment, call argument, return, constructor field,
and collection element. This is payload-aware `Optional`-to-`Optional`
assignability; it is **not** a widening: `Optional[int32]` is not assignable to
`Optional[int64]`, `Optional[bigint]` not to `Optional[int64]`, `Optional[str]`
not to `Optional[int64]`, and so on. The partner bare-payload rule is unchanged:
a bare payload value of type `T` still widens into `Optional[T]` (#1265,
`Optional[int64] x = n` for an in-range numeric `n`), because that value is not
itself an `Optional`.

## Rationale

Sema erases the `Optional` payload: it tracks every `Optional[T]` opaquely as
`Named("Optional")` (`resolve_type` / `is_optional_ty`), so to sema `Optional[U]`
and `Optional[T]` are the *same* type and `assignable` returns true for any `U`,
`T`. An explicit `Optional.Some(v)` monomorphizes its payload from the argument —
`Optional.Some(g)` for a `bigint g` builds `Optional$bigint`, `Optional.Some(n)`
for an `int32 n` builds `Optional$int32` — and the binding to an `Optional[int64]`
slot is accepted by the erased check. The differently-monomorphized value then
reaches IR lowering, where `coerce` has no `Optional`-to-`Optional` arm (`from`
and `to` are both pointer-shaped structs, so it is a no-op `return v`), and the
`Optional$U` value is stored raw into the `Optional$T` slot. The two
monomorphizations disagree on physical layout (a niche pointer-payload
`Optional$bigint` / `Optional$str` versus a tagged `Optional$int64`, or a tagged
payload of a different width) and on method dispatch, so a later `is_some()` /
`unwrap()` reads garbage or raises — a memory-unsafe miscompile in a checked
program.

This is the exact sibling of #1379: that fixed a bare `bigint` *literal* flowing
into a fixed-width-numeric payload (lowering sees `bigint -> int*` and rejects
it), but a value whose `Optional` has already monomorphized `Optional$U` slips
that check — the `Optional$U` is not a bare `bigint`, it is a wrapped one. The
fix completes the payload check #1379 started: a numeric bare payload is
width-checked, and now a whole `Optional` is payload-checked.

The consistent behaviour is **reject**, not a checked re-wrap. Admitting a cross
-payload `Optional` would require unwrapping, coercing the payload (with its own
`None` handling) and re-tagging — a silent conversion the language offers nowhere
else, and for the `Optional[int32] -> Optional[int64]` case a genuine narrowing
risk. The user builds the target payload explicitly (e.g. `Optional.Some(int64(n))`,
or rely on the bare-payload widening `Optional[int64] x = n`).

## Placement

In IR lowering, not sema, for the same reason as #1379's `check_numeric_payload`:
sema has erased the `Optional` payload by the time it checks assignability
(`coercible`), while lowering has the concrete, monomorphized payload type
(`OptionalLayout::payload_ty`). The reject is one `check_optional_payload(from, to)`
helper (both are `optional_layout`s whose `payload_ty` differ), applied at the
**final destination** frame of `expr_collection_context` — the single chokepoint
every type-contextual position flows through (`expr_coerced` for bindings /
arguments / returns / fields, and the collection-element sites directly).

The check runs only at a *final* destination (`final_dest`), never at the internal
`Optional`-unwrap recursion: that recursion lowers against one payload level of an
outer `Optional`, so a correctly-formed *outer* `Optional` (e.g.
`Optional[Optional[int64]]`) legitimately appears there "one level too big" and is
passed up to be resolved by the outer frame's `it == target`. Checking it at that
non-final frame would false-positive on nested `Optional`s.

A consequence, shared with #1379: this specific error surfaces at `zap build` /
`zap ir`, not through the sema-only MCP `check_source` surface. Moving it earlier
would require threading the `Optional` payload type through sema — the broader
#1265 "verify the payload" follow-up (un-erase the `Optional` payload in
`coercible`), which this ADR does not take.

## Consequences

- No new ZIR `Inst` or runtime call; a rejected program fails to lower with a
  clear `ZAP4001` instead of miscompiling. Accepted programs are byte-identical: a
  matching-payload `Optional` (`Optional[bigint] x = Optional.Some(g)`,
  `Optional[int64] y = x` for an `Optional[int64] x`), a nested
  `Optional[Optional[T]]`, and the #1265 bare-payload widening are all unaffected.
- Interface covariance of an `Optional` payload (`Optional[Dog] -> Optional[Animal]`)
  is likewise rejected here: the niche representations differ (a raw `Dog` pointer
  versus an `{data, itable}` fat pointer), so it would be the same miscompile
  class. Admitting it with a proper re-box is a separate feature, not in scope.

<!-- Fix in crates/ir/src/lower.rs (`check_optional_payload`, called from the
     final-destination frame of `expr_collection_context_impl`). The over-accepting
     sema rule is the payload-erased #1265 widening / `assignable` identity at
     `coercible`. Sibling of `decisions/lang/1379-bigint-optional-payload.md`. -->
