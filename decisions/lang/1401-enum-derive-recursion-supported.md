---
title: "Enum @derive recursion through an enum-typed variant payload is supported"
status: Accepted
issue: 1401
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Enum `@derive` recursion through an enum payload (#1401)

**Status:** Accepted — records that `#1401` is **not** a core defect and pins the
supported behavior with a golden and a host test.

## Finding

`#1401` was filed during the `#1375` breadth slice claiming that a method call on
an enum-typed variant-pattern capture (the generated `$cvp{i}` payload capture,
`#1374`) failed with `error[ZAP6001]: method call on a non-struct value`, and that
this blocked enum→enum recursion in a `@derive` codec. On investigation the claim
does not hold: the capture **does** dispatch its method correctly. A generator
whose variant payload is itself a derived enum lowers end-to-end, and the outer
specialization dispatches through the inner enum's derived method on its
enum-typed capture (`@_derive_sum$Outer` → `@Inner__sum`).

The original repro failed for a different reason: its enum arm unconditionally
called `.sum()` on **every** variant payload field, including a scalar leaf
(`int64 a`), which carries no `sum` method — a generator-authoring omission, not
a language gap. The `ZAP6001` receiver was the `int64` leaf, never an enum
capture.

## Canonical idiom

A recursive enum generator distinguishes a scalar leaf from a recursable payload
with a **per-field comptime kind check** inside the variant arm, the counterpart
of the top-level kind dispatch from `#1394`:

```zap
case comptime v in reflect.type_info(T).variants as payload:
    int64 acc = 0
    comptime for f in v.fields:
        if f.ty.kind.name == "struct" or f.ty.kind.name == "enum":
            acc = acc + reflect.field(payload, comptime f.index).sum()
        else:
            acc = acc + reflect.field(payload, comptime f.index)
    return acc
```

This spans a struct, an enum with a scalar-leaf variant, and an enum nesting
another enum from one `@derivable` generator.

## Guard

- Golden `tests/golden/enum_derive_enum_recurse.zap` (runtime output under the
  CI LLVM matrix): `Outer.O(Inner.Leaf 42)` → `42`, `Outer.O(Inner.Pair P(3,4))`
  → `7`, plus the two inner-enum cases directly.
- Host test `an_enum_derive_recurses_through_an_enum_typed_variant_payload`
  (`zap_cli`): the program lowers without `ZAP6001`, both enums specialize, and
  the outer specialization emits `call @Inner__sum` on its enum-typed capture.

## Consequence

Enum→enum recursion is a supported part of the enum-`@derive` surface (`#1371`);
the `#1375` scope-out of it was based on an incomplete generator and is corrected
here. The only remaining enum-`@derive` breadth gap is generic-enum construction
(`#1402`).
