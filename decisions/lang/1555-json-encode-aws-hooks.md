---
title: "E5: library @derive(Json) encode for the AWS @json field hooks"
status: Accepted
issue: 1555
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Library `@derive(Json)` encode for the AWS `@json` hooks (#1555)

**Status:** Accepted — the E5 (#1551) encode slice that reproduces the AWS `@json`
field hooks (ticket #346, D-0086) in the library `JsonBytes` generator, byte-
identically to the built-in. These hooks feed the generated AWS SDK, so the
retire slice (#1558) cannot land until the library matches them. The built-in is
left intact.

## The hooks and how the library reproduces them byte-for-byte

- **`bytes` → base64** (type-driven, no annotation). Built-in:
  `zap_base64_encode(v, urlsafe=0, pad=1)` + `zap_json_escape`. Library:
  `json.dumps(JsonValue.Str(base64.encode(v)))`. `base64.encode` is the standard
  alphabet, padded (`crates/sema/src/modules/base64.rs`), the same wire form; the
  output is ASCII, so `json.dumps` only quotes it — identical to `zap_json_escape`.
- **`DateTime` → epoch | ISO-8601**, selected per field by `@json(timestamp=...)`.
  Built-in: `zap_datetime_to_unix` (epoch, a JSON number) / `zap_datetime_format_iso`
  (ISO-8601, a JSON string). Library: `datetime.to_timestamp(dt)` (→ `int64`, so
  `json.dumps(JsonValue.Int(...))` is a bare integer matching the built-in) and
  one-arg `datetime.format(dt)` (→ `2023-11-14T22:13:20`, verified against the
  `datetime_strftime` golden). No annotation ⇒ epoch, matching the AWS awsJson
  default.

## Reading `@json(timestamp=)` at comptime — the mechanism this slice proves

The timestamp form is read off the field's decorator through **nested
`comptime for`** over `f.attrs` (→ `AttrInfo`) and `a.args` (→ `AttrArg`,
`.is_named` / `.name` / `.value`). This golden is the first to exercise nested
comptime attribute iteration inside a derive generator, and it lowers clean.
`@json(name=)` rename stays out — the built-in never wired it (`JsonFieldOpts`
carries only `timestamp`), so it is not part of byte-parity.

## Scope

Top-level `bytes` / `DateTime` fields (the AWS SDK shape). An `Optional`- or
collection-wrapped hook field encodes its payload through the same leaf once the
Optional/collection arms (#1554 / #1559) and these arms meet; that composition is
covered when decode (#1557) and the retire regression wall (#1558) exercise the
real AWS goldens.

## Proof + CI gate

`tests/golden/derive_json_encode_aws_hooks.zap` asserts
`s.to_json() == s.to_jsonbytes()` for a `bytes` field (`foobar`, empty, non-UTF-8
binary), and a `DateTime` field in epoch and ISO-8601 forms at two instants. `zap
ir` lowers clean and `zap fmt --check` is clean locally; byte equality is gated
by the CI LLVM-18 matrix (Docker/LLVM down). The existing `json_aws_blob` /
`json_aws_timestamp` goldens remain the built-in byte oracle.
