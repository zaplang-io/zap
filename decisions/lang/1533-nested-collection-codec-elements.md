---
title: "Built-in @derive(Json) supports nested-collection codec elements"
status: Accepted
issue: 1533
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Nested-collection codec elements (#1533)

**Status:** Accepted — the fourth and final slice of the collection-fields epic
(#1529, under E4 #1429), after Json `list` (#1530) / `dict` (#1531) and Yaml
parity (#1532). The built-in `@derive(Json)` codec now handles a collection
whose element/value is itself a collection: `list[list[T]]`,
`dict[str, list[T]]`, `list[dict[str, T]]`, nested to any depth, with scalar /
str / bytes / nested-struct leaves.

## Design

The `list` / `dict` codec arms already recover a scalar element's read-back `Ty`
via `list_elem_json_ty` / `dict_value_json_ty` and recurse through
`gen_json_encode_field` / `gen_json_decode_field`. A nested-collection element is
represented as `Elem::Nested(id)` (#625: a flat `Elem::Ptr` would lose whether a
list element is a `str` or a `list[int64]`). The only change here is: those two
helpers now take `ctx` and, for `Elem::Nested(id)`, recover the element's full
`Ty` from the interner (`ctx.nested.borrow().get(id)`) instead of rejecting it.
The existing recursion then does the rest — reading/writing each nested element
as a pointer word (`Elem::Nested.val_suffix() == "ptr"`, the proven #625
read-back path) and validating the inner element in turn (so a nested
`dict[int64, …]`, a `set`, or a tuple leaf is still rejected cleanly by the
recursion).

## Scope

- **Json only.** A yaml collection flow-encodes through `gen_json_encode_field`
  and can bottom out in a struct leaf whose `to_json` `@derive(Yaml)` does not
  emit, so `yaml_collection_elem_ok` still rejects a nested-collection (and
  nested-struct) element — a yaml collection stays flat (scalar / str / bytes).
- **No `Optional` in value position.** `list[Optional[T]]` /
  `dict[str, Optional[T]]` remain a clean "nested Optional payload" error (the
  existing `gen_json_encode_field` / decode guard) — a value-position optional is
  a separate follow-up, not part of this epic.

## Testing

Golden `tests/golden/derive_json_nested_collection_field.zap` round-trips
`list[list[int64]]`, `dict[str, list[int64]]`, `list[dict[str, int64]]` and
`list[list[Item]]` (struct leaf), proven by nested index/key inspection (a
nested-collection-field struct can't `@derive(Eq)`, D-0049; dict order is
unspecified, #1066), plus the empty case. Host tests assert it lowers + recurses
into a struct leaf's codec, and that a yaml nested-collection element still
errors cleanly. Local Docker/LLVM is down, so the runtime round-trip is gated by
the CI LLVM-18 matrix.

With this slice, epic #1529 is complete: the built-in `@derive(Json)` codec
supports scalar / str / bytes / nested-struct / `Optional` / `list` / `dict`
fields with arbitrary collection nesting, and `@derive(Yaml)` matches it for flat
collections.
