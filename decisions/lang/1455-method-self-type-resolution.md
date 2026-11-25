---
title: "A method's Self return / parameter resolves to the enclosing type in codegen"
status: Accepted
issue: 1455
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Method `Self` type resolution in codegen (#1455)

**Status:** Accepted — fixes a core defect where a method whose declared return
(or non-`self` parameter) is `Self` was not resolved to the enclosing type in IR
lowering, so the result typed as a non-struct and a field access on it failed
with `error[ZAP6001]: field access on a non-struct value`.

## Cause

IR builds each method's signature by resolving its declared types with
[`resolve_ty`]. The `self` *parameter* was special-cased to `Ty::Struct(sid)`,
but the **return type** and any **non-`self` parameter** went through
`resolve_ty` unchanged — which has no `Self` case, so `Self` failed to resolve
and `unwrap_or` degraded it to `Ty::Void` / `Ty::Ptr`. A call `p.m()` to a
`-> Self` method therefore produced a non-struct value, and chaining a field
access (`p.m().x`) hit `ZAP6001`. This affected **any** `Self`-returning method —
hand-written or a library-derived stub (it surfaced building the codec-decode
path, whose `from_json(…) -> Self` must type as the deriving struct, #1454).
sema already resolved `Self` correctly (`sig_of_method` / `resolve_type_self`);
only the codegen signature did not.

## Fix

A small `subst_self(ty, self_name)` rewrites every `Self` in an AST type to the
enclosing type's name (recursing through generic arguments, tuples, and function
types), applied before `resolve_ty` at each method return site (struct, tier-3
enum, tier-1 enum) and for non-`self` parameters. Letting the ordinary resolver
see the concrete name means `Self` lands on the correct `Ty` for every kind —
`Ty::Struct` for a struct or a tier-3 enum, `Ty::Enum` for a tier-1 enum — with
no special case inside `resolve_ty`.

## Guard

- Golden `tests/golden/method_self_return.zap`: a hand-written `clone(self) ->
  Self` used bare and chained (`p.clone().x`), and a `Self`-typed parameter
  (`max_x(self, Self other) -> Self`) — runtime output `3 / 4 / 3 / 9` (CI LLVM
  matrix).
- Host tests `a_method_returning_self_is_usable_at_the_call_site` and
  `a_derived_method_returning_self_resolves_to_the_deriving_type` (the latter a
  `reflect.construct`-based decoder, #1454).

## Consequence

`Self`-returning methods work for field access and chaining, hand-written or
derived. Combined with the reflect-construct enabler (#1454), a library codec
**decoder** (`… -> Self`) is now expressible end-to-end — the remaining blocker
for a codec-decode shadow slice is the encode side (ordered-object JSON build /
source JSON-escape).
