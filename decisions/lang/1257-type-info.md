---
title: Compile-time type introspection — reflect.type_info(T) (read-only, E1)
status: Accepted
issue: 1257
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# Compile-time type introspection — `reflect.type_info(T)` (read-only, E1)

**Status:** Accepted — this ADR records the *implemented shape* of phase **E1** of
the compile-time reflection epic (#1248). The resolved design and its rationale
live in Discussion #1243 (all nine hard questions). E1 is the read-only
introspection layer: the smallest shippable slice, independently useful, and the
prerequisite for the E2 comptime evaluator (#1250) and E3 `@derive`.

## Decision

A type's *shape* is exposed as a read-only compile-time value, `TypeInfo`, with:

- `kind` — one of `Struct | Enum | Tuple | Primitive | List | Dict | Optional`.
- `name` — the type name for a user `struct`/`enum` (nominal) and the canonical
  spelling for a primitive (`int64`, `bool`, `str`, `char`, `float64`, …);
  `None` for the structural container kinds (`Tuple`/`List`/`Dict`/`Optional`).
- `fields` — a struct's declared **data fields only**, in **declaration order**
  (#1066). Each `FieldInfo` carries `name`, resolved `ty`, declared type
  `decl_ty`, 0-based `index`, `attrs`, and `span`.
- `variants` — an enum's variants, in declaration order. Each `VariantInfo`
  carries `name`, `index`, payload `fields` (as `FieldInfo`, empty for a
  unit variant), `attrs`, and `span`.
- element/key/value types for the structural kinds: `List{elem}`,
  `Dict{key, value}`, `Optional{elem}`, `Tuple{elems}`.

Reflection is **nominal** for user struct/enum and **structural** for the
built-in containers, and is **one level, lazily recursive** (Discussion #1243
HQ1a): a field/element carries its own type, which can be reflected again — the
resolver never flattens the type graph, so it is finite on recursive types.

E1 is **sema-only**: no codegen, no runtime materialization, no comptime
evaluator. The value model lives in `crates/sema_types`; the resolver
(`Checker::reflect_type_info`, exposed through `zapc_sema::analyze_reflect` ⇒
`Reflector`) reads the struct/enum registries and is pure/read-only. It is
assert-tested host-side (`crates/sema/tests/reflect_type_info.rs`), which is how
the E2 evaluator (#1250) will eventually drive it. Consuming the value
(indexing, `comptime for`, typed `reflect.field`, the kind-switch, `@derive`) is
later work (#1250/#1251/#1252/#1253).

### Implemented representation

- The sema struct table was enriched: `StructInfo.fields` went from a bare
  `Vec<Ty>` to `Vec<FieldInfo>`, sourced from the AST (names, attributes, and
  spans already existed there). `EnumVariantInfo` likewise went from
  `Vec<(String, Ty)>` payload pairs to `Vec<FieldInfo>` plus a variant `span`.
  Every field now carries both its resolved canonical `ty` *and* the declared
  `decl_ty` — see *Optional* below.
- `type_info(T)` is a single positional type argument. It is **left positional**
  (no kwargs): `seam_sigs.rs` is unchanged and there is no `seam_kwargs` entry,
  because a one-type-argument query has nothing to bind by name. (There is no
  callable surface in E1 to bind against yet, either.)
- `FieldInfo.attrs` is modeled now (HQ9), even though nothing consumes it until
  E3: an `AttrInfo { name, args, span }` per field decorator, each `AttrArg`
  being an optional keyword `name` plus the argument rendered to a canonical
  string `value` (a string literal without its quotes, a numeric lexeme,
  `true`/`false`, a bare name, or `…` for a richer expression). The compiler only
  *captures* attributes as structured data; it assigns them no meaning (that is
  library policy, E3).
- An unsupported/opaque type — a container outside the E1 kind set
  (`set`/`heap`/`deque`/`oset`/`channel`/`AtomicArray`/…), a function type, a
  Python handle, a compiler-synthesized `$…` type, or an un-inferable type —
  resolves to a `ReflectError` whose message is exactly
  `` `<type>` is not available for introspection `` anchored at the type's
  span. It is **never a panic**. In E1 this surfaces only to the host test; E2
  will raise it as a real, span-attributed compile error at the comptime call
  site (and, with E3, a stable `ZAP####` code — HQ4).

## Rationale

- **Enrich the existing table, don't add a parallel one.** The field names,
  attributes, and spans reflection needs already live in the AST; the sema
  struct/enum registries simply dropped them. Promoting `fields` to `FieldInfo`
  keeps one source of truth and lets the value-struct auto-hashable check and the
  `@scoped` contagion read `field.ty` unchanged.
- **Resolve on the declared type, keep the resolved type too.** The resolved
  `Ty` is the canonical identity the rest of the compiler uses, but it erases an
  `Optional[T]`'s element (sema tracks `T?` opaquely as `Named("Optional")`).
  Keeping `decl_ty` alongside `ty` lets reflection report an optional/container
  element correctly without widening the core `Ty` model (which would be a large,
  risky blast radius across every `match` on `Ty`).
- **Kind set = exactly the design's.** `Struct/Enum/Tuple/Primitive/List/Dict/
  Optional` are the kinds Discussion #1243 enumerated and the ones the serde/
  `@derive` driver needs. Other containers (`set`/`heap`/`deque`/`oset`/`array`/
  `channel`/…) resolve to the "not available for introspection" diagnostic rather
  than being silently described; a later phase extends the set when a consumer
  needs one (sets serialize as sequences, but nothing consumes that yet).
- **A `TypeShape` enum, not a bag of options.** Each kind carries exactly — and
  only — the data it can have, so a `match` over a `TypeInfo` is exhaustive and
  an impossible combination (a list with variants, a struct with a key type) is
  unrepresentable.
- **No surface wired yet.** There is no `comptime` keyword, no `reflect` stdlib
  module, and no callable `reflect.type_info` in E1 — those need the evaluator
  (E2) to be useful. Wiring a half-working surface now would either overreach
  into E2 or ship something unusable. The read-only model and resolver are the
  genuinely independent slice.

## Consequences

- The reflection model and resolver are in place for E2 (#1250) to evaluate and
  for E3 to build `@derive` on; `analyze_reflect`/`Reflector` are the host entry
  points.
- `StructInfo.fields` / `EnumVariantInfo.fields` are now richer records; every
  in-tree reader was migrated to `field.ty`. No cross-crate consumer existed.
- `Optional`'s element is recovered only from a directly-written `Optional[T]`;
  a bare `Optional`, or an alias to one, reports element `Unknown`. Container
  recursion deeper than one level (e.g. an optional *inside* a reflected field)
  surfaces when that field's own type is reflected (one-level, lazily recursive).
- Enum variants and their payload fields have **no attribute grammar** today, so
  `VariantInfo.attrs` and enum-payload `FieldInfo.attrs` are always empty; the
  shape is in place for when a grammar is added.
- Deferred to later phases (not built here): the comptime evaluator and
  `comptime for` (#1250/#1251), typed `reflect.field` (#1252), the container
  kind-switch (#1253), `@derive` (E3), a `reflect` stdlib module and any runtime
  materialization of `TypeInfo`.

<!-- Related: epic #1248; design Discussion #1243; field/param reflection #458. -->
