---
title: "comptime: a reflected type argument in an explicit type-argument call"
status: Accepted
issue: 1575
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# A reflected type argument in a derive generator (#1575)

**Status:** Accepted — a core comptime fix (epic #1248), maintainer-assigned.
It lets a derive generator **name an element type through reflection** in an
explicit type-argument call: `gen[f.ty.elem](...)` for a `list` element,
`gen[f.ty.value](...)` for a `dict` value, `gen[f.ty](...)` for a field.

## The gap

A generator could *read* a nested derived collection element (encode recurses
via a method call on a known value, `x.to_json()`), but could not *construct /
decode* one: a decoder must invoke the element type's generic decoder, and there
was no way to name that type. Two forms both failed:

- `acc.append(gen(json.dumps(jv)))` → `ZAP6001 cannot infer type argument 'T'`
  (an `append` / index-assign gives inference no destination to anchor on, unlike
  `reflect.set_field` on a typed field, #1490).
- `gen[f.ty.elem](json.dumps(jv))` → `ZAP6001 unknown variable 'gen'`: the
  reflected type-argument subscript was never rewritten, so the callee was read as
  a value being indexed.

Explicit type-argument calls with a **syntactic** type (`gen[Item]()`,
`make[P]()`) already work; only a **reflected** type argument from a comptime
`Value::Type` was unsupported.

## The fix (one site, comptime specialization)

`crates/comptime/src/unroll.rs::specialize_expr`: when an `Expr::Index` subscript
folds to a `Value::Type(_, Some(decl))` — a comptime type carrying its use-site
declared `AstType` (#1346) — the subscript is rewritten to `Expr::TypeArg { ty:
decl }`. `mono::expr_to_type` already unwraps an `Expr::TypeArg` to its type
(#1488), so the enclosing generic call resolves against the concrete element type
after specialization. The whole generator still lowers through the existing
explicit-type-argument-call path — no new codegen.

Only a **type-valued** subscript is rewritten; an ordinary runtime index
(`list[i]`, `dict[k]`, `array[dtype]`) never folds to a `Value::Type`, so this is
inert for all other subscripts. When `decl` is absent (a bare `Optional` or a
top-level `reflect.type_info(T)` with no AST in hand) the rewrite does not fire
and behavior is unchanged.

## Why it matters

It unblocks the E5 (#1551) `@derive(Json)` library **decode** for
`list[DerivedStruct]` / `dict[str, DerivedStruct]` (the case #1557 deferred), and
is the mechanism E6's serde `deserialize` needs to recurse into a container
element by type. It is the decode/construct counterpart to the method recursion
an encoder uses: a decoder must *name* the element type, which only reflection
can supply inside a generic generator. (Enum-variant and generic-struct
*construction* remain the separate gap #1561.)

## Tests

- `crates/driver/tests/zap_cli.rs::reflected_type_argument_resolves_in_a_derive_generator`
  — full pipeline: a generator using `gen[f.ty.elem]` / `gen[f.ty.value]` over
  `list[Item]` / `dict[str, Item]` compiles, with the element generator
  monomorphized for `Item` (host, no LLVM).
- `tests/golden/derive_json_decode_nested_collection.zap` — the E5 end-to-end
  proof: a library `@derive(Json)` decoder round-trips `list`/`dict` of a nested
  derived struct, agreeing with the built-in by keyed inspection (CI LLVM matrix).
