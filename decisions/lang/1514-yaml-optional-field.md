---
title: "@derive(Yaml) supports a top-level Optional[T] field (and no longer miscompiles it)"
status: Accepted
issue: 1514
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `@derive(Yaml)` and `Optional[T]` fields (#1514)

**Status:** Accepted — a correctness fix plus the small feature it implies, giving
`@derive(Yaml)` parity with `@derive(Json)` (I#165 / D-0065) on `Optional[T]`
fields. Extends the Yaml codec (I#149 / D-0063).

## The defect

`@derive(Yaml)`'s encoder (`gen_yaml_to_method`, `crates/ir/src/lower.rs`)
dispatched each field on `derive_field_kind(fty)` and, for **any** `Nested(fsid)`,
emitted a call to `{child}__to_yaml` and recursed. A monomorphized `Optional[T]`
lowers as the named aggregate `Optional$T`, which `derive_field_kind` classifies
as `Nested` — but `Optional$T` is not a `@derive(Yaml)` struct, so no
`Optional$T__to_yaml` is ever synthesized. The field therefore lowered to a call
to a **non-existent function**: an unresolved external symbol that the host build
could not see and only the full LLVM-18 / aarch64 matrix caught (as
`call to unknown function 'Optional$int64__to_yaml'`).

`@derive(Json)` and `@derive(Csv)` did not have this bug — Json handles a
top-level `Optional` one level up and guards a nested `Optional` payload
(`gen_json_encode_field`, with a comment naming this exact miscompile), and Csv
rejects every non-scalar cleanly. Yaml was the only one of the three that recursed
into a `Nested` field without first excluding the `Optional` aggregate.

Discovered during E4 #1490 / PR #1505 (widening `reflect.construct`'s zero-set):
the library Yaml-codec shadow derived `@derive(Yaml)` on a struct with an
`Optional` field and the merge-queue LLVM matrix caught the unresolved symbol. Per
the core-defect golden rule, #1490 scoped `Optional` out of the shadow rather than
working around it, and reported this.

## The fix

In `gen_yaml_to_method`, test `optional_layout(ctx, fty)` **before** the `Nested`
recursion:

- A **top-level `Optional[T]`** field now encodes directly (the implied feature,
  matching Json's D-0065 behavior): `fname: <inner>` when `Some` (a scalar inline,
  a nested `@derive(Yaml)` struct as an indented block — the same two shapes a
  non-optional field uses), and `fname: null` when `None`. The field's fragment is
  buffered through a one-slot `alloca` so the running accumulator (`acc`, threaded
  linearly and dominating the join) is appended exactly once regardless of arm.
- A **nested `Optional` payload** (`Optional[Optional[...]]`) is rejected with the
  clean diagnostic `@derive(Yaml) does not support a nested Optional[...] payload
  yet …`, mirroring `gen_json_encode_field`, instead of recursing into the
  non-existent encoder.

**Decode is untouched.** `from_yaml` already routes through the shared JSON
`from_json_value` core (`__yaml_loads` → `Struct__from_json_value`), which decodes
an `Optional` field from an **absent key or an explicit `null`** to `None`
(`gen_json_decode_optional`, I#165). The YAML loader maps `null` / `~` to the
`JsonValue.Null` variant, so `fname: null` round-trips to `None` with no decode
change. This is why the fix is encode-only.

## Why `null` on `None` (not key-omit)

Json omits the key on `None` (D-0065). Yaml emits an explicit `fname: null`
instead: it keeps `to_yaml` producing a non-empty mapping even when every field is
`None` (avoiding a `yaml.load("")` edge case), is idiomatic YAML, and round-trips
through the same absent-key-*or*-null decode path. Both encoders therefore decode
identically; only the wire text differs, and byte-identical cross-codec output was
never a goal.

## Scope

- Supported `Optional[T]` payloads: scalar, `str`, and a nested `@derive(Yaml)`
  struct (encode). Decode of a nested-struct payload remains bounded by
  `reflect.construct` only in the *library shadow*; the built-in `from_yaml`
  decodes a nested struct payload fine.
- A nested `Optional` payload and other non-scalar payloads (collections / enums)
  are clean "unsupported yet" errors — the documented follow-up, not a workaround.

## Tests

- Host (catches the defect without LLVM):
  `derive_yaml_lowers_a_top_level_optional_field_without_a_dangling_symbol`
  (the struct's `to_yaml` lowers and emits no `Optional$T__to_yaml`) and
  `derive_yaml_rejects_a_nested_optional_payload_cleanly` (the clean diagnostic).
- Runtime golden `yaml_derive_optional` (CI LLVM matrix): round-trips a scalar and
  a nested-struct `Optional` payload, both `Some` and `None`.
