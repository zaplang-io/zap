---
title: "E4 shadow-proves the @derive(Csv) codec over the @derivable mechanism"
status: Accepted
issue: 1480
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Shadow-prove the `@derive(Csv)` codec (#1480)

**Status:** Accepted — the second E4 (#1429) **codec** dogfood slice, after Json
(#1470). It proves the library `@derivable` mechanism can express a CSV **encode
and decode**, agreeing with the built-in `@derive(Csv)` by round-trip, **without
replacing** the built-in. Same approach as the scalar/structural shadows (Eq
#1431, Ord #1443, Hash #1444) and the Json codec shadow (#1470).

## Why Csv is a distinct proof from Json

Json already proved a recursive codec — but Json has a **source-accessible DOM**:
the `JsonValue` enum and public `json.dumps` / `json.loads` live in the prelude,
so a library generator builds and walks structure with no compiler-private
helper. CSV has **neither a public `csv` module nor a `__csv_*` prelude helper**:
the built-in codec is done entirely in codegen over private runtime
(`zap_csv_join` / `zap_csv_split` / `zap_csv_quote` / `zap_bool_to_str` /
`zap_str_to_i64` / `zap_str_to_f64` / `zap_str_to_bool`). So the shadow must
express the whole codec in **pure Zap source** with ordinary string primitives:

- encode: `str()` on `int64` / `float64` / `bool` (bool → `True` / `False`,
  matching the built-in's Python-csv convention), a `str` field as its own
  column, then `",".join(parts)`.
- decode: `s.split(",")`, then per column `int()` / `float()` / a manual bool
  parse / the `str` verbatim.

A feasibility pass confirmed every primitive exists and round-trips
(`string_convert.zap`, `conversions.zap`, `string_methods.zap`). This makes Csv
the slice that shows the mechanism reaches a built-in with **no DOM or module to
lean on** — only the public string surface and the two E4 enablers.

## The two enablers (as in Json)

- **`reflect.construct` / `reflect.set_field`** (#1454) — the reflection *write*
  surface the decoder builds a value with.
- **Derived-method `Self`-return typing** (#1455) — the decoder is
  `from_csvcodec(str) -> Self`, whose result types as the deriving struct at the
  call site.

## What this slice adds

Two single-method library traits, each with its own name so neither collides with
the built-in `Csv` registration in `DERIVABLE_TRAITS` (a trait currently maps to
exactly one generator, `derive_synth.rs`, so encode and decode are separate
traits):

- **`CsvEncode`** → `to_csvcodec(self) -> str`. The generator renders each field
  to its column string via `f.ty.name` comptime dispatch (`int64` / `float64` /
  `bool` → `str()`, `str` verbatim) and returns `",".join(parts)`.
- **`CsvDecode`** → `from_csvcodec(str) -> Self`. The generator splits the record,
  zero-constructs with `reflect.construct(T)`, and writes each field with
  `reflect.set_field(out, comptime f.index, …)`, parsing the column with ordinary
  source conversions (`int()` / `float()` / a `_csv_bool` helper — no `__csv_*`
  internals).

The forwarding stub for a no-`self` interface method (`from_csvcodec(str s)`)
synthesizes an **associated function** on the struct (#1260), callable as
`Row.from_csvcodec(s)` — the shape the built-in `from_csv` also uses.

## The proof is round-trip, cross-checked both directions

The golden `tests/golden/derive_csv_codec_shadow.zap` proves round-trip,
cross-checked against the built-in `@derive(Csv)` (which stays **untouched**):

- `Row.from_csvcodec(r.to_csvcodec()) == r` — library encode → library decode.
- `Row.from_csvcodec(r.to_csv()) == r` — library decode agrees with built-in
  encode.
- `Row.from_csv(r.to_csvcodec()) == r` — built-in decode agrees with library
  encode.

Two rows cover both bool values, a negative int and distinct floats. Because v1
restricts str values to no embedded delimiter, both encoders produce the same
plain comma-joined record (the built-in's quoting never triggers), so the
cross-direction checks are exact.

Host test `a_library_generator_shadows_the_built_in_csv_codec_without_replacing_it`:
built-in `@Row__to_csv` / `@Row__from_csv` (untouched) **and** shadow
`@Row__to_csvcodec` / `@Row__from_csvcodec` + `@_derive_csvencode$Row` /
`@_derive_csvdecode$Row` are all emitted.

**No wiring changes.** `DERIVABLE_TRAITS`, `gen_csv_to_method` /
`gen_csv_from_method`, and the prelude are all unchanged.

## v1 field-kind scope

- `int64` / `str` / `bool` / `float64` — every field kind a flat CSV record
  supports. Nested structs / collections / enums are rejected by the built-in
  `@derive(Csv)` itself (a CSV record is flat), so the shadow has no wider surface
  to cover and no "unsupported yet" field kind beyond the built-in's own.
- **Str values** in v1 carry no embedded comma / quote / newline. Full RFC-4180
  quoting / escaping of embedded delimiters is a documented follow-up — the
  built-in `zap_csv_quote` / `zap_csv_split` handle it in runtime; the pure-source
  shadow would need a manual quoter / parser. Not worked around: the slice simply
  scopes to the unquoted record, where both encoders agree exactly.

## Consequence

The `@derivable` mechanism is shown to express a real codec — encode and decode —
over a built-in that has **no source-accessible DOM or module behind it**, using
only the public string surface. Together with Json (#1470) this leaves **Yaml**
as the last codec slice (it shares the JSON DOM decode core, so it is closer to
Json than to Csv). RFC-4180 quoting is the one named follow-up for full-fidelity
CSV str values.
