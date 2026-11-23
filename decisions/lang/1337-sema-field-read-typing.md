---
title: Sema types a struct field read as the field's declared type
status: Locked
issue: 1337
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# Sema types a struct field read as the field's declared type

**Status:** Locked

## Decision

The checker's value-position inference for an attribute access (`Expr::Attr` on
a value base) now returns the accessed field's declared type when the base is a
struct that declares a field of that name. Concretely, in `infer`'s `Expr::Attr`
arm, once the base type `b` is known, `struct_info(&b)` is consulted and, if it
has a field named `name`, that field's resolved `Ty` is returned. The existing
special cases are unchanged and still take precedence: a qualified enum
member/variant (base is a type name), an enum value's `.name`, a built-in module
constant, and a `Ty::Py` handle. An attribute the struct does not declare still
falls through to `Ty::Unknown` (no new "unknown field" diagnostic is
introduced).

## Rationale

The arm previously handled only the cases above and fell through to
`Ty::Unknown` for every other base — including a `Ty::Named(struct)`. So a field
read was untyped, which `Ty::Unknown`'s "assignable to/from anything" rule turned
into silently-accepted type mismatches whenever a value flowed through a field:
`int64 x = self.some_list_field` was not flagged, while the identical mismatch
through a local binding was (`ZAP4001`). The field-read span was also never
recorded in the `TypeMap` the monomorphizer reads (#1167), which is the hole that
made #1329/#1339 reach mono as `Unknown`.

The monomorphizer already resolves a field read's type (`Mono::field_ty`); sema
was simply missing the symmetric rule. The fix mirrors it, making sema the single
source of truth for field-read types (consistent with #1169).

Generics are fail-closed by construction: the `Ty` model carries no type
arguments (`Ty::Named(String)`), and `resolve_type` maps a bare type-parameter
name to `Ty::Unknown` (it is not a `user_type`). So a generic struct's
parameter-typed field (`T item`) is stored as `Ty::Unknown` in `StructInfo`, and
this change returns `Unknown` for it — exactly the conservative pre-fix behavior.
Precise generic-field typing would require the `Ty` model to carry type
arguments; that is a separate, larger change and is deliberately **out of scope**.

Blast radius was measured before locking: type-checking the entire
`tests/golden` + `tests/conformance` + `tests/diff` fixture corpus (844
programs) produces the **same** errors with and without this change — zero new
diagnostics — and the full host test suite (sema, mono, ir, driver) is
unchanged. The only programs that error are pre-existing negative fixtures and
decorator-recognition artifacts of the standalone harness, identical on both
sides.

## Consequences

- A type mismatch reached through a struct field read is now a clear error at the
  read site (assignment `ZAP4001`, argument `ZAP4021`, operators via their usual
  codes), closing a class of silently-accepted mismatches. Correct field reads,
  including chained (`self.inner.v`) and container (`self.xs : list[T]`) reads,
  are unchanged.
- The field-read span now carries the field's type in the analysis `TypeMap`, so
  the monomorphizer's sema backfill (#1167) can pin a generic argument derived
  through a field read, not only through a local binding.
- Generic-struct parameter-typed field reads remain `Unknown` (fail-closed); no
  `Ty`-model change is made here.
- No new "unknown attribute" diagnostic: an undeclared attribute still infers
  `Unknown`, so method-value and dynamic-shaped reads are unaffected.

<!-- Related: [lang/1169-mono-consumes-sema-types](1169-mono-consumes-sema-types.md) (sema TypeMap backfill), [lang/1329-for-loop-element-type-binding](1329-for-loop-element-type-binding.md) (the mono-side fix whose investigation surfaced this gap). -->
