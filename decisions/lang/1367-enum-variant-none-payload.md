---
title: An enum-variant constructor argument lowers through the type-contextual chokepoint
status: Locked
issue: 1367
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# An enum-variant constructor argument lowers through the type-contextual chokepoint

**Status:** Locked

## Decision

An enum-variant constructor argument `Enum.Variant(arg)` is lowered against the
variant field's declared type through the shared `FnLower::expr_coerced` →
`expr_collection_context` chokepoint, exactly as struct-constructor fields
(`fill_struct_fields`, #1075), call arguments, and returns already are. A bare
`None` argument therefore reaches `lower_none_expr` — producing the canonical
empty value for the field type (the tagged `None` of a nested `Optional`, the
null of a reference, or a clear `None is not a valid <ty>` type error for a
non-nullable scalar) — and a bare payload auto-wraps into `Some` where sema
accepts it (#1262). Both the niche `Some` path and the tagged `build_variant`
path in `crates/ir/src/lower/enums.rs` now use `expr_coerced` instead of a
typeless `self.expr(arg)` + `coerce`.

## Rationale

The two enum-variant lowering paths were the last constructor positions still
lowering their payload with a bare `self.expr`, so a bare `None` argument fell
through to the generic `ZAP6001` "Phase 1 slice" bail (sema had already accepted
it). This blocked constructing a nested `Optional` — `Optional.Some(None)` for an
`Optional[Optional[T]]`, a legitimate outer-present / inner-absent value — and
any enum variant carrying an `Optional[T]` (or other reference) field with a bare
`None`. It is the same chokepoint-miss class as #1362 (bare payload
reassignment) and #1075/#1080/#1195 (bare `None` in struct fields / list / tuple
positions). Reusing the shared mechanism keeps the behaviour identical to
struct-constructor fields and introduces no new ZIR `Inst`, rooting entry, or
raising runtime call.

## Consequences

- `Optional.Some(None)` for `Optional[Optional[int64]]` / `Optional[Optional[str]]`
  and `E.V(None)` for an enum field of type `Optional[T]` now lower and run; a
  bare `None` for a non-nullable scalar variant field now gives the clear
  `None is not a valid ...` error instead of the opaque `ZAP6001`.
- A nullable-payload `Optional` (e.g. `Optional[Optional[T]]`) is represented
  tagged, not niche-optimized, so `Some(None)` is a distinct, well-formed value
  (verified host-side and in the `enum_variant_none` golden). The niche
  optimization is only applied where the payload cannot be null, so no
  `Some(None)`-vs-`None` collision is introduced.
- Out of scope: a single-level `Optional[str].Some(None)` (a `Some` of a null
  `str`) still collapses to the niche `None`; that is a semantically-dubious
  construction, left as-is.

<!-- Fix in crates/ir/src/lower/enums.rs (`lower_variant_construct` niche path
     and `build_variant`), reusing expr_coerced / lower_none_expr / gen_opt_some
     from crates/ir/src/lower.rs. Mirrors #1075 / #1262 / #1362. -->
