---
title: "E5: library @derive(Json) encode for the AWS open string enum"
status: Accepted
issue: 1556
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Library `@derive(Json)` encode for the open string enum (#1556)

**Status:** Accepted — the E5 (#1551) slice that reproduces the AWS open-enum
JSON **encode** (`@json(open)`, ticket #346, D-0086) in the library generator,
byte-identical to the built-in `gen_open_enum_json`. The built-in is untouched.

## The open-enum wire form, reproduced byte-for-byte

- A known (payload-free) variant → the JSON string of its **name**
  (`Vanilla` → `"Vanilla"`).
- The `Unknown(str value)` fallback → the JSON string of its **carried value**
  (`Unknown("Mango")` → `"Mango"`), forward-compatible with AWS adding values.

The generator consumes the enum with the reflected-variant match form (#1374):
`case comptime v in reflect.type_info(T).variants as payload`, which expands to
one arm per variant. A payload-free variant (`len(v.fields) == 0`) emits its
comptime `v.name`; the payload-carrying variant emits `reflect.field(payload, 0)`.
Each is wrapped by `json.dumps(JsonValue.Str(...))`, so quoting matches the
built-in `zap_json_escape`. No new comptime primitive — this is the read surface
#1371/#1374 shipped.

## DECODE is blocked on core gap #1561 — reported, not worked around

Open-enum **decode** (`from_json`: a JSON string → a known variant, else
`Unknown(raw)`) must **construct** an enum variant from the generator's type
parameter `T`. Probed against main:

- `reflect.construct(T)` on an enum → `ZAP6001` ("not a non-generic struct");
- `T.Variant(...)` / `T.Unknown(s)` in the generator → `ZAP3001` ("unknown name
  `T`"). (#1402 fixed `Box[int64].Variant(...)` — naming a *generic enum* with
  explicit type args — not constructing from a generator's *type parameter*.)

There is no reflection primitive to construct a variant selected at comptime.
This is exactly **#1561** ("generalize `reflect.construct` to enum + generic-struct
instances"), already filed under #1248 by the E6 serde session, which hit the same
wall: enum *serialize* is feasible today, *deserialize* is blocked. Per the
golden rule, this slice **stops at the encode half** and does not implement a
workaround. Consequence for the program: the retire slice (#1558) cannot retire
the built-in enum `from_json` until #1561 lands; the built-in enum decode stays
the oracle meanwhile.

## Proof + CI gate

`tests/golden/derive_json_encode_enum.zap` asserts `e.to_json() == e.to_jsonbytes()`
for both known variants, the `Unknown` fallback, and an empty carried value. `zap
ir` lowers clean and `zap fmt --check` is clean locally; byte equality is gated by
the CI LLVM-18 matrix (Docker/LLVM down). The existing `json_aws_open_enum` golden
remains the built-in byte oracle.
