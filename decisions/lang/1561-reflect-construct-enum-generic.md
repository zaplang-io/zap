---
title: "comptime: construct enum variants and generic-struct instances by reflection"
status: Accepted
issue: 1561
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Construct enums + generic-struct instances by reflection (#1561)

**Status:** Accepted — a core comptime fix (epic #1248), maintainer-assigned. It
generalizes the reflection **write/construct** surface so a derive generator can
build the two value shapes a decoder needs and `reflect.construct` (#1454/#1490)
could not: an **enum variant** and a **generic-struct instance**.

## Part A — enum variant construction

New primitive `reflect.construct_variant(T, comptime index, payload…)`: build the
`index`-th variant of enum `T`, the enum counterpart of `reflect.construct`. A
decoder selects the variant in a `comptime for v in reflect.type_info(T).variants`
loop and builds it by `comptime v.index`; the payload values are the variant's
fields.

- **sema** (`crates/sema/src/lib.rs`): `construct_variant` joins `construct` /
  `set_field` as a `reflect.*` comptime call of statically-unknown type (its
  first two arguments are a type and a comptime index; the payload is checked at
  monomorphization, like `set_field`'s value).
- **mono** (`crates/mono/src/lib.rs::rewrite_reflect_construct_variant`): resolve
  `T` to a concrete non-generic enum, resolve the comptime index to the variant,
  bounds-check the index and payload arity, and emit `ConcreteEnum.Variant(payload…)`
  — a bare `ConcreteEnum.Variant` for a payload-free variant — the existing
  enum-variant construction form. No new codegen. A generic enum instance is a
  clean "not supported yet" error (a further extension).

This is the primitive E5's open-enum `@derive(Json)` **decode** (#1556) needed and
that E6 serde `deserialize` needs for enums. It is the decode/construct
counterpart to the reflected-variant *match* form (#1374) an encoder uses.

## Part B — generic-struct instance construction

`reflect.construct(T)` now accepts a **generic instance** (`Pair[int64, str]`),
not only a non-generic struct. `rewrite_reflect_construct` resolves `T`; when it
is `Named { name, args }` with args, `reflect_construct_generic` substitutes the
instance's type arguments into each field type (`subst_type` + `subst_map`),
zero-builds each with `zero_expr`, requests the instance, and emits
`Name$args(zero(f0'), …)`. A field that substitutes to *another* generic instance
is `zero_expr`'s existing "unsupported yet" error (a nested-generic follow-up),
not a crash.

## Why one issue, both parts

#1561 was filed (by the E6 session) as the single missing *construct* surface
with these two shapes. Both reuse the same resolution/emit machinery and close
the decode side of the reflection model: `reflect.construct` /
`reflect.construct_variant` are the write duals of `reflect.field`.

## Tests

- `crates/driver/tests/zap_cli.rs`: `reflect_construct_variant_builds_an_enum_variant`
  (open-enum decode compiles) and `reflect_construct_builds_a_generic_struct_instance`
  (host, no LLVM).
- Goldens (CI LLVM matrix): `derive_json_decode_enum.zap` — a library open-enum
  `@derive(Json)` decoder agreeing with the built-in; `reflect_construct_generic.zap`
  — `reflect.construct(Pair[A, B])` zero-builds two distinct instantiations.

## Program effect

Closes the last E5 (#1551) retire blocker for enum `from_json`; the collection-
element struct-decode blocker was #1575. With both landed, the E5 retire (#1558)
is reachable, and E6 `deserialize` gains enum + generic-struct construction.
