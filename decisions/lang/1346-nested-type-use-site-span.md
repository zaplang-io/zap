---
title: "A reflected nested type carries its use-site span, not a canonical one"
status: Accepted
issue: 1346
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Nested-type use-site spans in comptime reflection (#1346)

**Status:** Accepted — the enabler for the multi-level derive `required because
of …` chain (E3d #1293, epic #1289). A `@derivable` generator can now point
`comptime.error` at a *nested* type — the `Widget` element of
`list[Widget] items`, not just the whole field — and the diagnostic underlines
exactly that element.

## Problem

`field.span` (a `FieldInfo`, #1256) points at a whole field, but a nested type
reached through the reflect model (`field.ty.elem`, `.key`, `.value`, `.inner`,
`.elems`) carried **no source span**. A serde-style generator that rejects a
list *element* could attribute the failure only to the field, and the multi-level
`required because of` chain had no per-level anchor for its intermediate frames.

Simply adding a `.span` to the canonical `TypeInfo` is wrong: that record is
resolved by name through the memoized registry (#1253/#1255) and is structural
and span-free, so two fields of the same type (`list[Widget]` here and there)
share one cached record. A use-site span cannot live on a shared, canonical
record — it is a property of each *reference*, and must come from the field's
declared `Type` subtree (`crates/syntax` keeps spans on every `Type` node).

## Decision

The use-site lives on the **reference value**, not the canonical record. The
comptime `Value::Type` gains a second component: the declared `AstType` this
reference came from, when known.

- `field.ty` resolves from the field's declared type and records it as the
  use-site, so `field.ty.span` is the field's declared-type span.
- The container accessors (`.elem`/`.key`/`.value`/`.inner`/`.elems`) **descend
  into the declared type's argument subtree** rather than re-resolving a
  span-free `Ty`: `list[Widget].elem` yields `Widget` with `Widget`'s own
  use-site span. This composes to arbitrary depth —
  `list[list[Widget]].elem.elem.span` is the innermost `Widget`, and
  `dict[str, list[Widget]].value.elem.span` likewise.
- A reference with no declared type in hand — a top-level `reflect.type_info(T)`,
  or a nested type recovered from a span-free `Ty` when no AST is available —
  carries no use-site, and `.span` is a clean comptime error, never a bogus span.

The canonical `TypeInfo` and its memo are untouched (still structural,
span-free, interned once per type). `Type::span()` is added to `crates/syntax`
as the uniform accessor.

## Consumer (mono)

`enrich_derive_error` already attributed a derive failure to the field whose
span **equals** the cause span. It now also matches the field whose declared
type **contains** the cause span (half-open byte-range containment), so a
generator that points at a nested element still yields
`required because of field `items` of `Bag``, with the primary error underlining
the element. The full per-container-level chain (one frame per nesting level)
remains E3d #1293; this change ships the span it needs and the one-level
attribution on top of it.

## Consequence

A generator reads `comptime.error(f.ty.elem.span, …)` and the compiler renders
one diagnostic with two distinct spans on one line — the element use-site as the
root cause, the enclosing field as the `required because of` frame — with no
comptime-internal symbol leaked. Regression-guarded by
`crates/comptime/tests/nested_span.rs` (every accessor, arbitrary depth, the
no-span case) and `crates/driver/tests/zap_cli.rs`
(`a_derive_can_point_at_a_nested_element_use_site`, end-to-end on the host).
