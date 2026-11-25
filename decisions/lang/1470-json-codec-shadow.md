---
title: "E4 shadow-proves the @derive(Json) codec over the @derivable mechanism"
status: Accepted
issue: 1470
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Shadow-prove the `@derive(Json)` codec (#1470)

**Status:** Accepted — the first E4 (#1429) **codec** dogfood slice. It proves the
library `@derivable` mechanism can express a JSON **encode and decode**, agreeing
with the built-in `@derive(Json)` by round-trip, **without replacing** the
built-in. Same approach as the scalar/structural shadows (Eq #1431, Ord #1443,
Hash #1444).

## Why a codec is the interesting slice

Eq/Ord/Hash/Repr fold over fields with polymorphic operators. A codec is the
first shadow that needs the two E4 enablers end-to-end:

- **`reflect.construct` / `reflect.set_field`** (#1454) — the reflection *write*
  surface a decoder builds a value with.
- **Derived-method `Self`-return typing** (#1455) — a decoder is
  `from_jsoncodec(str) -> Self`, whose result must type as the deriving struct at
  the call site.

It also leans on the JSON DOM already being **source-accessible**: the `JsonValue`
enum and the public `json.dumps` / `json.loads` live in the prelude, so a library
generator builds and walks the DOM with no compiler-private helper and **no core
change for encode**.

## What this slice adds

Two single-method library traits, each with its own name so neither collides with
the built-in `Json` registration in `DERIVABLE_TRAITS` (a trait currently maps to
exactly one generator, `derive_synth.rs`, so encode and decode are separate
traits):

- **`JsonEncode`** → `to_jsoncodec(self) -> str`. The generator builds a
  `JsonValue.Object` from the fields — per-kind wrapping via `f.ty.kind.name` /
  `f.ty.name` comptime dispatch (`int64`→`Int`, `str`→`Str`, `bool`→`Bool`,
  `float64`→`Float`), a nested derived struct recursing through its own
  `to_jsoncodec()` re-parsed with `json.loads` so it nests as structure — then
  `json.dumps(...)`.
- **`JsonDecode`** → `from_jsoncodec(str) -> Self`. The generator parses with
  `json.loads`, zero-constructs with `reflect.construct(T)`, and writes each field
  with `reflect.set_field(out, comptime f.index, …)`, reading the field's
  `JsonValue` off the decoded object with ordinary source helpers (a `match` on
  the DOM — no `__json_*` prelude internals).

The forwarding stub for a no-`self` interface method (`from_jsoncodec(str s)`)
synthesizes an **associated function** on the struct (#1260), callable as
`Flat.from_jsoncodec(s)` — the shape the built-in `from_json` also uses.

## The proof is round-trip, not byte-identical

Byte-identical encode is explicitly **out of scope**: `JsonValue.Object` is an
unordered `dict`, so key order is unspecified. The golden
`tests/golden/derive_json_codec_shadow.zap` proves order-insensitive round-trip,
cross-checked against the built-in `@derive(Json)` (which stays **untouched**):

- `Flat.from_jsoncodec(p.to_jsoncodec()) == p` — library encode → library decode.
- `Flat.from_jsoncodec(p.to_json()) == p` — library decode agrees with built-in
  encode.
- `Flat.from_json(p.to_jsoncodec()) == p` — built-in decode agrees with library
  encode.
- `Nested.from_json(o.to_jsoncodec()) == o` — a nested derived struct: library
  encode (which recurses) round-trips through the built-in decoder.

Host test `a_library_generator_shadows_the_built_in_json_codec_without_replacing_it`:
built-in `@Flat__to_json` / `@Flat__from_json` (untouched) **and** shadow
`@Flat__to_jsoncodec` / `@Flat__from_jsoncodec` + `@_derive_jsonencode$Flat` /
`@_derive_jsondecode$Flat` are all emitted, the encode recurses through the nested
struct (`@_derive_jsonencode$Inner`), and the nested struct synthesizes no decoder.

**No wiring changes.** `DERIVABLE_TRAITS`, `gen_json_to_method` /
`gen_json_from_method`, and the prelude are all unchanged.

## v1 field-kind scope

- **Encode**: `int64` / `str` / `bool` / `float64` + a nested derived struct.
- **Decode**: `int64` / `str` / `bool` / `float64`. A **nested-struct** field in
  decode is a clean "unsupported yet" error, *not worked around*: `reflect.construct`
  v1 zero-initializes only scalar / `str` / `bytes` fields (#1454), so the enabler
  itself rejects a nested-struct field. Widening that zero set (nested / Optional /
  collections) is the documented follow-up, after which nested-struct decode drops
  out for free.
- Collections / enums / `Optional` fields are follow-ups (mirroring the built-in
  codec's own field-kind restrictions).

## Consequence

The `@derivable` mechanism is shown to express a real recursive codec, encode and
decode, over the compiler's own JSON DOM — the hardest shadow so far, and the
template for the Csv and Yaml codec slices (which share the DOM decode core).
`reflect.construct` zero-set widening is the one named follow-up for full-fidelity
decode.
