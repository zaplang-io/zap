---
title: "Generic enum construction with explicit type arguments"
status: Accepted
issue: 1402
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Generic enum construction with explicit type arguments (#1402)

**Status:** Accepted — closes the last enum-`@derive` breadth gap (#1371): a
generic enum can now be constructed with an explicit type argument and a variant
path, `Box[int64].Full(9)` / `Box[int64].Empty`.

## Problem

`Box[int64].Full(9)` failed with `error[ZAP3001]: unknown name 'Box'` (and
`'int64'`). Generic *struct* construction (`Foo[int64](…)`), generic enum
*declaration* and derive-stub *synthesis* (#1373), and *inferred* generic enum
construction (`Box.Full(x)`, where the payload pins `T`) already worked. What was
missing was the **explicit-type-argument construction expression**: the parser
produces `Attr { base: Index { base: Name("Box"), index: int64 }, name: "Full" }`
(a call for a data variant, a bare attribute for a payload-free one), and sema had
no arm for it, so the base `Box[int64]` was type-checked as a value subscript —
rejecting `Box` and `int64` as unknown names. When no variant mentions `T`,
inference cannot recover the instance, so the explicit form is the only way to
name it and it was unusable.

## Decision

Recognize the explicit-type-argument form in **sema**, before the base is
resolved as a value, in the two positions the parser produces:

- the call form `G[args].Variant(payload)` in `check_call`, type-checked by the
  existing `check_variant_construction`;
- the payload-free form `G[args].Variant` in the attribute value-inference arm,
  mirroring the bare `Color.Red` member access.

Both are **gated to generic enums** (tracked in a `generic_enums` set, the sema
counterpart of the monomorphizer's `genums`): a type-argument subscript on a
non-generic enum stays an error. The variant name and payload arity are checked
exactly as the bare `Enum.Variant` forms are (`ZAP4060` unknown variant,
`ZAP4061` data variant used payload-free).

No lowering change was needed: the monomorphizer already lowers
`G[args].Variant(…)` via `type_args_from_index`, requesting the concrete instance
(`Box$int64`). The block was unreachable only because sema rejected the form
first.

Type-argument **arity** is not validated in sema, matching the existing
generic-struct construction path (`Foo[int64](…)`); a mismatch is handled during
monomorphization. Making arity strict is a separate, cross-cutting change (it
would also tighten structs) and is out of scope here.

## Consequence

Generic-enum `@derive` works end-to-end: a generic enum value can be built and
its derived method called (`Box[int64].Full(9).sum()`). This closes the enum
breadth epic #1371 — both scoped-out cases (enum→enum recursion #1401, generic
enum construction #1402) are resolved. The dogfood port of the built-in derives
(E4) is the next phase.
