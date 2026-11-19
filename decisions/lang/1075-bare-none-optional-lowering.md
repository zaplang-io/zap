---
title: Type-contextual bare `None` lowering is one shared mechanism
status: Locked
issue: 1075
pr:
date: 2026-11-19
domain: lang
supersedes:
superseded-by:
---

# Type-contextual bare `None` lowering is one shared mechanism

**Status:** Locked

## Decision

A bare `None` literal that reaches IR lowering with only its destination type to
go on — a struct-constructor field, a defaulted field, a call argument / return,
or a collection element (list/dict literal, list comprehension, `.append()`
value, index/key assignment) — is lowered through **one** chokepoint,
`FnLower::lower_none_expr(target)`:

- `Optional[T]` destination → the canonical empty option via `gen_opt_none`
  (tagged `None` for a scalar payload, niche null for a reference payload), so
  `is_some()`/`is_none()`/`unwrap_or` never dereference a stray null (the #564
  invariant, now shared across every type-contextual position);
- a plain reference destination (reference struct, `str`, a collection) → the
  null pointer (member access then throws per D-0007/EXC-4);
- a value struct → rejected (value types are not nullable, D-0014);
- a **non-reference** destination (scalar `int`/`float`/`bool`/`char`/enum) →
  rejected with a clear type error (`None is not a valid \`<ty>\` value`) rather
  than the opaque "expression not supported by codegen (Phase 1 slice)" bail.

The constructor-field path (`fill_struct_fields`, positional + keyword + omitted
default) now lowers each field through `expr_coerced`, matching field assignment
(`obj.field = value`), which already did. The collection-element paths route the
element through `expr_collection_context`, which consults `lower_none_expr`
first; list comprehensions gained an expected-element-kind variant
(`lower_list_comp_expected`) so an annotated `list[Optional[T]]` destination
threads its element type into the comprehension body.

## Rationale

Before this, a type-contextual bare `None` was handled in exactly one place
(`expr_coerced`, added for #564) and nowhere else, so two positions that never
flowed through it — a constructor field (#1075) and a `list[Optional[T]]` element
(#1080) — hit the generic lowering bail (`ZAP6001`, "Phase 1 slice"). Sema typed
them correctly as `Optional[T]`; only lowering lacked the context.

Three local special cases (one per path) were rejected in favour of a single
shared helper: the `None` representation question (tagged vs niche vs null vs
rejected) is identical in every position, and duplicating it is how #564's gap
survived in the constructor and list-element paths in the first place.
Centralizing it means any future type-contextual position only has to reach the
one helper. The comprehension needed a genuinely new capability (threading the
expected element kind), but it still funnels its element through the same
`expr_collection_context` → `lower_none_expr` path, not a parallel one.

## Consequences

- No new ZIR `Inst`/`Elem`/`Expr`/`Ty` variant, so no new GC-rooting
  (`inst_dest`) entry and no new exhaustive-match arms: `lower_none_expr` only
  emits the pre-existing `StructNew`/`FieldSet`/`NullConst` that #564 already
  wired for rooting, and performs no raising runtime call.
- Constructor fields now behave like annotated bindings and field assignments:
  a bare `None`, a `fn(...)` lambda, and a nested collection literal in a field
  all lower against the declared field type. This is a strict superset of the
  old behaviour (which only handled scalars/structs via `expr` + `coerce`).
- A bare `None` for a non-nullable field/element is still rejected (it was an
  opaque `ZAP6001` before; it is now a clear message, same `ZAP6001` code).
- Out of scope / unchanged: the pre-existing `int`-vs-`int64` strictness of
  list-element representation (`list[Optional[int]] = [Optional.Some(5)]` still
  requires the `Some` literal's repr to match the declared element); and `None`
  elements in `set`/`dict` comprehensions (the benchmark ports that motivated
  this use lists). Those are separate follow-ups if they arise.

<!-- Related: the #564 call-argument/return fix this generalizes lives in
     crates/ir/src/lower.rs `expr_coerced` / `gen_opt_none`. -->
