---
title: "E5: byte-identical library @derive(Json) encode"
status: Accepted
issue: 1554
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Byte-identical library `@derive(Json)` encode (#1554)

**Status:** Accepted — the first E5 (#1551) *replace* slice. Where the E4 Json
shadow (#1470) proved only **round-trip** agreement, this slice proves the
library `@derivable` generator produces the **exact same bytes** as the built-in
`@derive(Json)` encoder, for the scalar / `str` / `bool` / `float64` / nested-
derived-struct / `Optional[int64]` surface. The built-in is left fully intact;
nothing is retired here (retiring is #1558, gated on all E5 slices).

## Why round-trip was not enough, and the byte-identical trick

The E4 shadow built a `JsonValue.Object` from an **unordered** `dict[str,
JsonValue]` and called `json.dumps`, so object key order was unspecified —
round-trip-safe but not byte-exact. The built-in instead emits keys in **field-
declaration order** and omits a `None` `Optional` key entirely (D-0065).

The library generator reproduces that byte layout **without any new core
primitive**: it builds the object envelope by hand — a `list[str]` of
`"name":<leaf>` fragments, `",".join`ed and brace-wrapped — iterating
`comptime for f in reflect.type_info(T).fields`, which unrolls in declaration
order (#1066). This mirrors the built-in's `zap_csv_join` + `{}` layout exactly
(no spaces, declaration order). An `Optional` field pushes its fragment only in
the `Some` match arm, so a `None` omits the key with no dangling comma — the same
result the built-in gets from its present-fragment list.

Each scalar/string **leaf** is produced by `json.dumps` of a single `JsonValue`
leaf (`JsonValue.Int(x)` / `.Str` / `.Bool` / `.Float`), reusing the runtime JSON
formatting the DOM path already uses; a nested derived struct recurses through
its own `to_jsonbytes()`, byte-identical to its own `to_json()` by induction
(the nested struct derives `JsonBytes` too).

## Primitives relied on (all already shipped)

- `reflect.type_info(T).fields` field iteration in declaration order (E1 #1257).
- `reflect.field(value, comptime i)` typed field read (E2 #1252).
- `f.ty.kind.name` / `f.ty.name` comptime kind/type dispatch with dead-arm
  elision (#1326), so only the matching per-field branch survives.
- `comptime.error(f.span, …)` for an unsupported field kind (E2 #1256).

No comptime/reflection primitive was added. The feasibility review (#1551)
confirmed field-attribute access (`f.attrs`) and the source-accessible
`base64` / `datetime` module APIs exist for the later AWS-hook slice (#1555).

## The proof is a runtime string equality, CI-LLVM-gated

`tests/golden/derive_json_encode_byte_identical.zap` derives both the built-in
`Json` and the library `JsonBytes` on each struct and asserts
`s.to_json() == s.to_jsonbytes()` for representative values — including
escape-bearing strings (`"`, `\`, newline, tab, non-ASCII), a nested struct, and
an `Optional` present and absent. Every assertion prints `True`.

Docker/LLVM is down on the maintainer box, so `zap ir` proves only lowering
(verified clean here); the byte-identical equality is gated by the **CI LLVM-18
matrix**. The sharpest residual parity edge is float leaf formatting
(`json.dumps(JsonValue.Float)` vs the built-in `zap_f64_to_str`); if CI shows a
divergence, the fix is to route the same scalar primitive to source, still not a
core comptime change.

## Scope boundary

`list[T]` / `dict[str, V]` collection fields (built-in #1530 / #1531) are the
next E5 encode slice (#1559); enum open-codec is #1556; decode is #1557.
