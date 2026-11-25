---
title: "Built-in @derive(Json) supports list[T] fields"
status: Accepted
issue: 1530
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `@derive(Json)` `list[T]` fields (#1530)

**Status:** Accepted — the first slice of the collection-fields epic (#1529,
under E4 #1429). The built-in `@derive(Json)` codec now encodes a `list[T]`
field as a JSON array and decodes a JSON array back into a list.

## Why

The built-in codecs supported scalars, `str`, nested structs and `Optional`;
`list` / `dict` were the last field-kind gap, and the gap is what kept the E4
codec *shadows* from covering collections. This slice adds `list` to the
built-in enabling layer (dict and Yaml parity are the sibling sub-issues #1531 /
#1532; nested-collection elements are #1533).

## Design

- **Classifier.** `DeriveFieldKind` (`crates/ir/src/lower.rs`) gains a
  `List(Elem)` variant; `derive_field_kind` returns it for `Ty::List(elem)`.
  `list_elem_json_ty` maps the element representation to its read-back `Ty`, as
  an **allowlist**: `int` (`Elem::I64`), `float` (`F64`), `str` (`Ptr`), `bytes`,
  and a nested struct (`Struct`). A `bool`/`char`/`enum`/`complex` element is
  rejected because those collapse to a coarse `i64` word through `Elem::ty()`
  (`Elem::val_suffix()` maps them to the `i64` slot), and a nested-collection
  element (`Elem::Nested`/`VarTuple`) needs the interner path — both deferred to
  #1533 with a clean error.
- **Encode** (`gen_json_encode_field`) string-builds `[e0,e1,…]`: iterate the
  list at runtime (`zap_list_len` + `zap_list_get_<suffix>`), recurse through
  `gen_json_encode_field` per element (so each element encodes exactly as the
  same type would at top level), comma-join (`zap_csv_join`) and bracket-wrap —
  the array analog of the object builder in `gen_json_to_method`.
- **Decode** (`gen_json_decode_field`) reads the DOM array via a new Zap-prelude
  accessor `__json_as_array` (added to `PRELUDE_SRC` in
  `crates/syntax/src/parser.rs`, mirroring `__json_field`; lenient `case _ -> []`),
  then loops and recurses through `gen_json_decode_field` per element, pushing
  each into a typed result list (`zap_list_push_<suffix>`). No C runtime change.
- The other codecs are unaffected: `@derive(Csv)` and `@derive(Yaml)` encode add
  the `List` kind to their existing reject arms (collections stay a follow-up
  there); `@derive(Yaml)` *decode* shares the `from_json_value` core, so a Yaml
  list decode already works through this path, but a Yaml list *encode* still
  errors — keeping `@derive(Yaml)` + list a clean compile error until #1532.

## Testing

A struct with a `list` field cannot `@derive(Eq)` (a list is not hashable,
D-0049), so the golden `tests/golden/derive_json_list_field.zap` proves the
round-trip by **field inspection** (encoded JSON string + decoded lengths and
indexed elements), the way `json_derive_optional.zap` does — covering a scalar
list, a str list, a nested-struct list, and the empty-list case. Host tests in
`crates/driver/tests/zap_cli.rs` assert it lowers and wires `__json_as_array` +
element recursion, and that an unsupported element (`list[list[...]]`,
`list[bool]`) and a `dict` field still error cleanly.

Local Docker/LLVM is down, so the runtime round-trip is gated by the CI LLVM-18
matrix; the host build confirms lowering + accessor wiring.
