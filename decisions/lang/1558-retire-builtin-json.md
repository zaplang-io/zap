---
title: "E5: retire the built-in @derive(Json), serve it from a prelude library codec"
status: Accepted
issue: 1558
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Retire the built-in `@derive(Json)` (#1558)

**Status:** Accepted — the capstone of E5 (#1551). `@derive(Json)` is no longer a
compiler built-in: it is served by a **prelude library `@derivable` codec**, and
the JSON-only built-in codegen + sema special-cases are deleted. This is the
first full *replace* (E4 was shadow-only), the template for E6's serde.

## How `@derive(Json)` is served now

`@derive(Json)` is **sugar** for deriving two single-method library traits
(`derive_synth::expand_json_derive_sugar`, pre-sema): `__JsonDeriveEncode`
(`to_json(self) -> str`) and `__JsonDeriveDecode` (`from_json(str) -> Self`), each
with a `@derivable` generator. The generators + interfaces + element/DOM helpers
live in `crates/syntax/src/json_derive.zap`, injected into a module only when it
routes a `@derive(Json)` (the #1596 prelude-aware mechanism;
`references_prelude_derivable` + `json_derive_items()`), so a non-Json program is
byte-for-byte unchanged. The trait names are internal (`__`-prefixed) so a user
file that itself declares `JsonEncode`/`JsonDecode` (e.g. the E4 shadow golden)
does not collide.

The codec is plain Zap over the source-accessible JSON DOM (`JsonValue`,
`json.dumps` / `json.loads`) plus `reflect.*`, built on the E5 enablers:
`reflect.type_info` field/variant iteration, `reflect.field`/`set_field`,
`reflect.construct` (+ enum-field / container widenings, #1603/#1608),
`reflect.construct_variant` (#1561), and reflected type-args in generic helpers
(#1575/#1604) — the last giving recursive element codecs for **nested
collections** (`list[list[T]]`, `dict[str, list[T]]`). It reproduces the built-in
byte-for-byte: field-declaration-order objects, `Optional` omit-on-`None`
(D-0065), `bytes`→base64, `DateTime`→epoch/ISO-8601 per `@json(timestamp=)`, and
the open string enum (`@json(open)`, now gated structurally by an `Unknown(str)`
variant rather than requiring the built-in marker).

## What was deleted

- **`crates/ir/src/lower.rs`**: the `DerivedTrait::Json` variant + all its arms;
  `gen_json_to_method`, `gen_json_from_method`; the open-enum path
  (`gen_open_enum_json`, `validate_open_enum`, the `enum_*_json` helpers,
  `json_enums` + its signature/codegen loops + the `@json(open)`-requires-
  `@derive(Json)` validation).
- **`crates/sema/src/lib.rs`**: `"Json"` dropped from `DERIVABLE_TRAITS`; the
  built-in `to_json` method synthesis removed.

## What was kept (shared with the still-built-in YAML codec)

`gen_json_from_value_method`, `gen_json_encode_field`, `gen_json_decode_*`,
`JsonFieldOpts` / `parse_json_field_opts` — the built-in `@derive(Yaml)` reuses
the JSON struct field encoder and the shared `from_json_value` decode core, so
these stay until YAML is also ported. A small, now-inert JSON `from_json`
dispatch remains in `check_call` (keyed on the retired built-in markers, so never
triggered; it returns the correct `Self` type and harmlessly coexists with the
library stub) — a trivial follow-up cleanup, not a behavior.

## Gate

Every pre-existing Json golden — `json_derive_to`/`from`/`optional`/`name_field`,
`json_aws_blob`/`timestamp`/`open_enum`, `derive_json_list_field`/`dict_field`/
`nested_collection_field`, `derive_json_codec_shadow`, `json_dumps`/`loads` — stays
**byte-identical** under the CI LLVM-18 matrix (the regression wall), now produced
by the library. Host-side: `zap ir` lowers every Json golden; a `zap_cli` test
(`derive_json_is_served_by_the_prelude_library_codec`) pins the routing; the YAML
codec and goldens are unaffected. Obsolete built-in-codegen unit tests (raw
`lower_module` / shape greps / stale rejection limits) were removed or switched to
the still-built-in `Csv`/`Yaml`, their behavior now covered by the goldens.
