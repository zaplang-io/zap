---
title: "E4 shadow-proves the @derive(Yaml) codec over the @derivable mechanism"
status: Accepted
issue: 1484
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Shadow-prove the `@derive(Yaml)` codec (#1484)

**Status:** Accepted — the third and **last** E4 (#1429) **codec** dogfood slice,
after Json (#1470) and Csv (#1480). It proves the library `@derivable` mechanism
can express a YAML **encode and decode**, agreeing with the built-in
`@derive(Yaml)` by round-trip, **without replacing** the built-in. Same approach
as the scalar/structural shadows (Eq #1431, Ord #1443, Hash #1444) and the Json /
Csv codec shadows.

## Yaml is the closest sibling of Json

YAML 1.2 is a JSON superset, so YAML reuses the **shared `JsonValue` DOM**: the
public `yaml` module exposes `yaml.load(str) -> JsonValue` and
`yaml.dump(JsonValue) -> str` (goldens `yaml_dump_load.zap`, `yaml_flow.zap`),
exactly as `json.loads` / `json.dumps`. The built-in `from_yaml` itself decodes
through the same `JsonValue` core as `@derive(Json)` (`__yaml_loads` →
`Struct__from_json_value`). So this shadow is the Json codec shadow (#1470) with
`json.dumps` / `json.loads` swapped for `yaml.dump` / `yaml.load` — the opposite
end of the spectrum from Csv (#1480), which had no DOM at all and was written in
pure string source.

## The two enablers (as in Json)

- **`reflect.construct` / `reflect.set_field`** (#1454) — the reflection *write*
  surface the decoder builds a value with.
- **Derived-method `Self`-return typing** (#1455) — the decoder is
  `from_yamlcodec(str) -> Self`, whose result types as the deriving struct at the
  call site.

## What this slice adds

Two single-method library traits, each with its own name so neither collides with
the built-in `Yaml` registration in `DERIVABLE_TRAITS` (a trait currently maps to
exactly one generator, `derive_synth.rs`, so encode and decode are separate
traits):

- **`YamlEncode`** → `to_yamlcodec(self) -> str`. The generator builds a
  `JsonValue.Object` from the fields — per-kind wrapping via `f.ty.kind.name` /
  `f.ty.name` comptime dispatch (`int64`→`Int`, `str`→`Str`, `bool`→`Bool`,
  `float64`→`Float`), a nested derived struct recursing through its own
  `to_yamlcodec()` re-parsed with `yaml.load` so it nests as structure — then
  `yaml.dump(...)`.
- **`YamlDecode`** → `from_yamlcodec(str) -> Self`. The generator parses with
  `yaml.load`, zero-constructs with `reflect.construct(T)`, and writes each field
  with `reflect.set_field(out, comptime f.index, …)`, reading the field's
  `JsonValue` off the decoded object with ordinary source helpers (a `match` on
  the DOM — no `__yaml_*` / `__json_*` prelude internals).

The forwarding stub for a no-`self` interface method (`from_yamlcodec(str s)`)
synthesizes an **associated function** on the struct (#1260), callable as
`Flat.from_yamlcodec(s)` — the shape the built-in `from_yaml` also uses.

## The proof is round-trip, not byte-identical

Byte-identical encode is explicitly **out of scope**: `JsonValue.Object` is an
unordered `dict`, so key (and thus `yaml.dump` line) order is unspecified. The
golden `tests/golden/derive_yaml_codec_shadow.zap` proves order-insensitive
round-trip, cross-checked against the built-in `@derive(Yaml)` (which stays
**untouched**):

- `Flat.from_yamlcodec(p.to_yamlcodec()) == p` — library encode → library decode.
- `Flat.from_yamlcodec(p.to_yaml()) == p` — library decode agrees with built-in
  encode.
- `Flat.from_yaml(p.to_yamlcodec()) == p` — built-in decode agrees with library
  encode.
- `Nested.from_yaml(o.to_yamlcodec()) == o` — a nested derived struct: library
  encode (which recurses) round-trips through the built-in decoder.

Host test `a_library_generator_shadows_the_built_in_yaml_codec_without_replacing_it`:
built-in `@Flat__to_yaml` / `@Flat__from_yaml` (untouched) **and** shadow
`@Flat__to_yamlcodec` / `@Flat__from_yamlcodec` + `@_derive_yamlencode$Flat` /
`@_derive_yamldecode$Flat` are all emitted, the encode recurses through the nested
struct (`@_derive_yamlencode$Inner`), and the nested struct synthesizes no decoder.

**No wiring changes.** `DERIVABLE_TRAITS`, `gen_yaml_to_method` /
`gen_yaml_from_method`, and the prelude are all unchanged.

## v1 field-kind scope

- **Encode**: `int64` / `str` / `bool` / `float64` + a nested derived struct.
- **Decode**: `int64` / `str` / `bool` / `float64`. A **nested-struct** field in
  decode is a clean "unsupported yet" error, *not worked around*: `reflect.construct`
  v1 zero-initializes only scalar / `str` / `bytes` fields (#1454), so the enabler
  itself rejects a nested-struct field. Widening that zero set (nested / Optional /
  collections) is the documented follow-up, after which nested-struct decode drops
  out for free — identical to the Json slice's decode boundary.
- Collections / enums / `Optional` fields are follow-ups.

## Consequence

The `@derivable` mechanism is shown to express a real recursive codec over the
shared YAML/JSON DOM, completing the E4 codec trio: **Json** (#1470, DOM + public
`json`), **Csv** (#1480, pure string source, no DOM), and **Yaml** (this, DOM +
public `yaml`). Together with the scalar/structural shadows (Eq / Ord / Hash /
Repr) all seven `DERIVABLE_TRAITS` built-ins now have a library shadow proving the
mechanism expresses them. `reflect.construct` zero-set widening is the one named
follow-up for full-fidelity codec decode.
