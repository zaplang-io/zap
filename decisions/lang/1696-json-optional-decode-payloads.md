---
title: "@derive(Json) decode handles any Optional[T] element payload"
status: Accepted
issue: 1696
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# `@derive(Json)` decode of `Optional[T]` element payloads (#1696)

**Status:** Accepted. Closes the single dominant blocker for the generated AWS
SDK (`zaplang-io/aws-codegen`, generated-is-product): **274 of 435** service
clients failed to compile with the same error — the prelude `@derive(Json)`
**decode** generator could not derive `__JsonDeriveDecode` for a struct with an
`Optional` field whose payload was anything other than `int64` or `str`.

## Background

`@derive(Json)` is a prelude library `@derivable` codec (#1558): a pair of
generators over the JSON DOM plus `reflect.*`, replacing the retired built-in
codegen. The **encode** generator already decoded an Optional field generically
— it recursed into the payload type through the shared recursive element codec
(`__json_enc_elem[f.ty.inner]`) and emitted the inner value, omitting the key
for `None`. The **decode** generator did not: it hard-coded two fast paths
(`f.ty.inner.name == "int64"` / `"str"`) and `comptime.error`-ed every other
payload with `@derive(Json): unsupported Optional payload`. That reject was a
`v1` placeholder (I#165/D-0065) that was never completed.

AWS shapes routinely carry an Optional field of a `float64`, `bool`, `list`,
nested `@derive(Json)` struct, or open string `enum` — so decode rejected the
shape even though encode compiled, blocking the whole client.

## Decision

Make decode **symmetric with encode**. The Optional-field decode arm recurses
into the payload through the same shared element codec
(`__json_dec_elem[f.ty.inner]`) and wraps the result in `Optional.Some`:

- an **absent key** or an **explicit JSON `null`** leaves the field at the `None`
  the zero-construct (`reflect.construct`) already gave it;
- any other value is `Some(decode payload)`.

Because decode now routes through the identical element codec as encode, an
Optional field supports **exactly** the payloads encode supports — scalars
(`int64`/`float64`/`bool`/`str`), `list` / `dict`, a nested `@derive(Json)`
struct, and an open string `enum` — guaranteeing the byte-identical round-trip
contract (#1558) holds for Optional fields on decode as well as encode.

## Scope boundary

`bytes` and `DateTime` carry bespoke **top-level** codecs (base64 and
epoch-or-iso8601) that the shared element codec deliberately does not reach — so
`list[bytes]`, `dict[str, DateTime]`, and an `Optional` *of* one are all
unsupported on both encode and decode. Rather than fall into the element codec
and emit a dangling symbol (or a confusing internal `reflect.construct` error), an
`Optional[bytes]` / `Optional[DateTime]` payload, and a nested `Optional`
payload, stay a **clean `unsupported Optional payload` reject**, preserving the
prior diagnostic contract. Full Optional-`bytes`/`DateTime` support (which also
needs the encode side) is a separate follow-up; the generated AWS SDK does not
need it (those fields never occur as Optional — encode would already have failed
on them).

## Testing

- `tests/golden/json_derive_optional_payloads` — an AWS-flavored `Resource`
  struct with Optional fields of `int64`, `str`, `float64`, `bool`, `list[str]`,
  a nested `@derive(Json)` struct, and an open string `enum`, round-tripped
  across present / absent / explicit-null, inspected by field (dict order is
  unspecified, #1066) and by encode round-trip (proving `None` omits its key).
- Host tests (`crates/driver/tests/zap_cli.rs`): the matrix lowers through the
  library decoder, and `Optional[bytes]` / `Optional[DateTime]` stay the clean
  reject (not a panic or internal error).
- The existing `json_derive_optional` golden (Optional `int64` / `str`) is
  unchanged — no regression on the former fast paths.

Local Docker/LLVM-18 is down, so the runtime round-trip is gated by the CI
LLVM-18 matrix; host `zap ir` proves lowering.
