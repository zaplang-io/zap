---
title: "Built-in @derive(Json) supports dict[str, V] fields"
status: Accepted
issue: 1531
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `@derive(Json)` `dict[str, V]` fields (#1531)

**Status:** Accepted — the second slice of the collection-fields epic (#1529,
under E4 #1429), following the `list` slice (#1530). The built-in `@derive(Json)`
codec now encodes a str-keyed dict as a JSON object and decodes a JSON object
back into a dict.

## Design (mirrors the list slice)

- **Classifier.** `DeriveFieldKind` gains `Dict(Elem, Elem)` (key and value
  representations); `derive_field_kind` returns it for `Ty::Dict(k, v)`.
  `dict_value_json_ty` requires the key to be `str` (`Elem::Ptr`) — a JSON object
  has string keys, so a non-str key (`dict[int64, V]`, …) is a clean error — and
  reuses the list element allowlist (`list_elem_json_ty`) for the value:
  int / float / str / bytes / nested `@derive(Json)` struct.
- **Encode** (`gen_json_encode_field`) string-builds `{"k":v,…}`: iterate the
  keys (`zap_dict_keys_ptr`), read each value (`zap_dict_get_str_<vsuf>`),
  JSON-escape the key (`zap_json_escape`), recurse through the per-value encoder,
  comma-join and brace-wrap — the object analog of the list arm.
- **Decode** (`gen_json_decode_field`) reads the DOM object via a new Zap-prelude
  accessor `__json_as_object` (added to `PRELUDE_SRC`, mirroring `__json_as_array`;
  lenient `case _ -> {}`), then iterates the object's keys, decodes each value
  through the per-value decoder, and sets it into a typed result dict
  (`zap_dict_set_str_<vsuf>`). No C runtime change.
- The `Dict` kind is added to the `@derive(Csv)` / `@derive(Yaml)` encode reject
  arms (collections stay a follow-up there, as for `list`).

## Notes

- Dict iteration order is unspecified (#1066), so the golden
  `tests/golden/derive_json_dict_field.zap` proves the round-trip by **keyed
  inspection** of the decoded value (indexing by key + lengths), not by printing
  the encoded object or by `==` (a dict-field struct can't `@derive(Eq)`,
  D-0049). Covers a scalar value, a str value, a nested-struct value, and the
  empty-dict case.
- Host tests assert it lowers + wires `__json_as_object` / `zap_dict_keys_ptr` +
  value recursion, and that a non-str key, a bool value, and a nested-collection
  value (`dict[str, list[...]]`) error cleanly.

Local Docker/LLVM is down, so the runtime round-trip is gated by the CI LLVM-18
matrix; the host build confirms lowering + accessor wiring.
