---
title: "Built-in @derive(Yaml) supports list / dict[str,V] fields (JSON-flow)"
status: Accepted
issue: 1532
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `@derive(Yaml)` `list` / `dict[str,V]` fields (#1532)

**Status:** Accepted — the third slice of the collection-fields epic (#1529,
under E4 #1429), after Json `list` (#1530) and `dict` (#1531). `@derive(Yaml)`
now encodes and decodes a `list[T]` / `dict[str, V]` field for scalar / `str` /
`bytes` element and value types.

## Design

- **Decode is already shared.** `Struct__from_yaml` parses with `__yaml_loads`
  (which transpiles the YAML subset to JSON and reuses `__json_loads`) and hands
  off to `Struct__from_json_value` — the same decode core `@derive(Json)` uses,
  which gained the `list` / `dict` arms in #1530 / #1531. So no decode change was
  needed; a yaml collection field decodes through that path.
- **Encode: JSON flow.** YAML 1.2 is a JSON superset, and the yaml encoder already
  reuses the JSON scalar formatters ("JSON scalars are valid YAML"). So
  `gen_yaml_to_method` emits a collection field as `fname: <flow>\n` where `<flow>`
  is the compact JSON array/object produced by `gen_json_encode_field`
  (`[e0,e1]` / `{"k":v}`). That is valid YAML and round-trips through
  `__yaml_loads`. This reuses the JSON element recursion and allowlist wholesale.
  A block sequence/mapping layout is a cosmetic follow-up.

## The one extra restriction: no nested-struct element

A nested-STRUCT element/value in a yaml collection is rejected
(`yaml_collection_elem_ok`). The flow encode of a struct element routes through
the element type's `{T}__to_json`, but `@derive(Yaml)` emits only `to_yaml` /
`from_yaml` / `from_json_value` — **not** `to_json`. So a struct element in a
yaml-only collection would be an unresolved symbol at codegen (the same
build-only failure mode as #1514's `Optional$T__to_yaml`). Scalar / str / bytes
elements need no `to_json`, so they are supported; a nested-struct element in a
yaml collection is a documented follow-up (it would need `@derive(Yaml)` to also
emit `to_json`, deduped against `@derive(Json)` like `from_json_value` already is).

## Testing

Golden `tests/golden/derive_yaml_list_dict_field.zap` proves the round-trip by
keyed/indexed inspection (a collection-field struct can't `@derive(Eq)`, D-0049;
dict order is unspecified, #1066), covering int / str lists and a str→int dict,
plus the empty case. Host tests assert it lowers + flows through the shared JSON
DOM accessors, and that a nested-struct element errors cleanly. Local Docker/LLVM
is down, so the runtime round-trip is gated by the CI LLVM-18 matrix.
