---
title: A plain local/global reassignment `name = value` wraps a bare payload as `Some(T)`
status: Locked
issue: 1362
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# A plain local/global reassignment `name = value` wraps a bare payload as `Some(T)`

**Status:** Locked

## Decision

A plain reassignment `name = value` to an existing `Optional[T]` binding — a
local slot or a module-level mutable global slot (D-0112) — lowers its
right-hand side through the shared `FnLower::expr_coerced` →
`expr_collection_context` chokepoint, exactly as constructor fields, field
assignment, index/key assignment, `.append`, call arguments and returns already
do. A bare payload `T` reassigned into an `Optional[T]` binding is therefore
boxed as a tagged/niche `Some(T)` via `gen_opt_some`, and a bare `None` lowers
to the binding's canonical empty option via `lower_none_expr`. This makes
reassignment the symmetric partner of the #1262 constructor/field/index wrap and
the #1075/#1080/#1195 bare-`None` lowering.

## Rationale

#1362 was a **memory-safety** bug in a *checked* program. The `Stmt::Assign` arm
for a `Name` target (`crates/ir/src/lower/stmts.rs`) lowered the right-hand side
with a bare `self.expr(value)` followed by `coerce`, so it was the one assignment
path that never reached the Optional-wrap chokepoint every other accepted
position used. Sema already **accepts** `offset = seconds` for an `Optional[int64]`
local (consistent with the bare-payload positions #1262 accepts), so the raw
`i64`/pointer was stored straight into the Optional slot with no discriminant tag
and no allocation. A later `offset.is_some()` / `offset.unwrap()` read those raw
bits as an Optional object and SIGSEGV'd — the issue's fault address
`0xffffffffffffd5d0` is the two's-complement bit pattern of the payload `-10800`.

The fix resolves the binding's declared type first (via `name_lvalue`) and then
lowers the value *against* that type with `expr_coerced` — the shared mechanism,
not a special case at the assignment site. `expr_coerced` also performs the
`fn(...)` structural signature check (I#465) and the final `coerce`, so the
non-Optional reassignments this arm already handled keep their exact behaviour.
An already-`Optional[T]` source is returned unwrapped (the `it == target` guard),
so it is never double-wrapped into `Some(Some(..))`; an incompatible source
passes through for `coerce`/sema to reject.

## Consequences

- No new ZIR `Inst`/`Elem`/`Expr`/`Ty` variant: the wrap reuses the pre-existing,
  already-GC-rooted `StructNew`/`FieldSet` (scalar payload) or slot-reinterpret
  (niche/pointer payload) that `gen_opt_some` already emits for an explicit
  `Optional.Some(...)`. No new `inst_dest` rooting entry, no new raising runtime
  call.
- Reassignment now emits the same `Some` box an explicit `Optional.Some(...)`
  would — verified by the host ZIR-structure tests in
  `crates/ir/tests/optional_reassign_lower.rs` (every store into the Optional
  binding is a whole-`Optional`-typed value, never a raw payload word) and the
  native-value `optional_reassign` golden (the tagged/niche-vs-raw storage only
  crashes once the native collector runs).

## Scope boundary

This extends the #1262 chokepoint to the last accepted-but-unwrapped position;
it changes no sema acceptance. The positions a bare payload is still **rejected**
in by sema (a `return`, an `Optional[T]` parameter, a typed binding
`Optional[T] x = v`, a list-literal element) are unchanged — the IR chokepoint
already wraps them defensively, so the implicit-widening language question
(#1265) needs no further lowering change if it later relaxes sema there.

<!-- Fix in crates/ir/src/lower/stmts.rs (`Stmt::Assign` -> `Expr::Name` arm),
     reusing expr_coerced / expr_collection_context / gen_opt_some from
     crates/ir/src/lower.rs. Mirrors #1262 / #1075 / #1195. -->
