---
title: "reflect value construction + field write (reflect.construct / reflect.set_field)"
status: Accepted
issue: 1454
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# reflect construct + field write (#1454)

**Status:** Accepted — the E4 (#1429) core enabler that gives comptime reflection
a **write** surface, the counterpart of the `reflect.field` **read** (#1252,
whose module notes "field write is deferred until a codec consumer needs it").
It lets a `@derivable` generator build a value of `T` and populate its fields —
the prerequisite for codec **decoders** (`from_json` / `from_csv` / `from_yaml`),
which the read-only surface could not express.

## Surface (field-set builder)

Chosen 2026-11-24 over a positional `reflect.construct(T, a0, …)` (can't
enumerate args over a `comptime for`) and a new comptime field-map construct (far
larger parser surface):

```zap
auto out = reflect.construct(T)                       # a zero-initialized T
comptime for f in reflect.type_info(T).fields:
    reflect.set_field(out, comptime f.index, <value>) # out.<name_i> = value
```

- **`reflect.set_field(target, comptime i, v)`** desugars to the ordinary field
  assignment `target.<name_i> = v` — the exact write-counterpart of
  `reflect.field(value, i)` → `value.<name_i>`. Done in the comptime layer
  (`desugar_set_field`, reusing `reflect_field` to resolve the LHS) during the
  `comptime for` unroll where the concrete field name is known.
- **`reflect.construct(T)`** desugars, at specialization (with `T` substituted to
  a concrete type), to `ConcreteName(zero(f0), zero(f1), …)` — each field its
  type's zero. Done in mono (`rewrite_reflect_construct`). The initial zeros are
  throwaway (a decoder overwrites every field with `set_field`); an uninitialized
  struct decl is not a thing and there is no zero-init primitive, hence the
  per-field zero literals.

Sema accepts both as comptime calls of statically-unknown type (no value
inference of the type argument), like `reflect.field` / `reflect.type_info`.

## v1 scope

`reflect.construct` zero-initializes a **non-generic struct** whose fields are
scalars / `str` / `bytes`; a generic instance or a nested-struct / Optional /
collection field is a clean "unsupported yet" error, mirroring the derive
codegen's field-type restrictions. Widening the zero set (nested/Optional/
collections) and generic instances are follow-ups.

## Guard

- Golden `tests/golden/reflect_construct_build.zap`: a generator zero-constructs,
  `set_field`s each field from a source list, and reads them back — sum `60`
  (runtime, CI LLVM matrix).
- Host test `reflect_construct_and_set_field_build_a_value`: lowers with no
  unresolved `reflect` / non-struct error; the specialized generator zero-inits
  and emits ordinary field stores.

## Known blocker for the decode consumer

A library-derived method returning **`Self`** is not typed as the deriving type
at the call site (`ZAP6001` on a field access of the result) — a pre-existing
gap, independent of this surface (reproducible with a generator that just returns
`self`), filed as **#1455**. A real decoder is `from_json(…) -> Self`, so #1455
must land before this enabler can be consumed for decode. The enabler itself is
complete and tested via a scalar-returning generator.
