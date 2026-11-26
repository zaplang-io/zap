---
title: "E6: serde breadth slice 4 — enum encode + constant-variant decode"
status: Accepted
issue: 1606
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# serde breadth slice 4 — `enum` (#1606)

**Status:** Accepted. Adds `enum` types to the bundled `serde` foundation
(#1570): encode of all variants (tag + payload), and decode of payload-free
(constant) variants via `reflect.construct_variant`. No per-codec compiler change.

## Top-level kind dispatch, pruned per instantiation

`serialize` / `deserialize` now branch on `reflect.type_info(T).kind.name`
(`"enum"` vs everything else). The branch is a comptime-foldable constant per
specialization, so a struct `T` never compiles the enum arm and an enum `T` never
compiles the struct arm. The struct body moved into `_serialize_struct` /
`_deserialize_struct` helpers, which are only instantiated for a struct `T` — this
matters for decode: `_deserialize_struct` calls `reflect.construct(T)`, which
errors on an enum, so the enum arm must `return` from the dispatcher *before* the
helper is referenced (hence the `else:`, not a trailing call).

## Backend surface (one method each direction)

- `serde.Serializer` += `emit_variant(self, str name)` — the variant tag; any
  payload values follow via the existing `emit_*`.
- `serde.Deserializer` += `read_variant(self) -> str` — reads the tag, before any
  payload.

## Encode — all variants

A `match value: case comptime v in reflect.type_info(T).variants as payload:`
emits `emit_variant(v.name)` then walks `v.fields` with a `comptime for`,
dispatching each payload field by kind (`int64` / `uint64` / `float64` / `str` /
`bool` / `bytes`). An unsupported payload kind is a `comptime.error`.

## Decode — constant (payload-free) variants this slice

`read_variant()` gives the tag; a `comptime for` over the variants matches it to a
`v.name` and returns `reflect.construct_variant(T, comptime v.index)`; an unknown
tag raises `ValueError`.

### Why payload-carrying decode is deferred (not a core gap)

`reflect.construct_variant(T, i, payload…)` exists and works (#1561). The blocker
is purely in a *generic* driver: each variant carries a different payload arity
and field types, and a single `deserialize[T, D]` body would have to spread a
heterogeneous, per-variant comptime payload list into the constructor call. That
is a codegen-shaped comptime-spread problem, not a missing primitive. This slice
rejects a payload-carrying variant at compile time with a clear `comptime.error`
pointing at the variant field's span; encode of payload variants is fully
supported. A follow-up slice can add payload decode once comptime payload-spread
lands.

## Proof

`tests/golden/serde_bundled_import.zap` serializes `Status.Paused` through the
real bundled `serde` (asserted token stream `Paused`, no struct frame) and decodes
`"Active"` back, matching `Status.Active`. Runtime is CI-LLVM-gated; the bundled
host test asserts the enum specialization (`serde__serialize$Status$Sink`,
`serde__deserialize$Status$Src`) and the backend calls (`emit_variant` /
`read_variant`). Widening the bundled interface ripples to backends;
`zaplang-io/msgpack` gets a matching update as a follow-up.

## Deferred

Payload-carrying enum decode (above); enum of a nested struct / collection payload.
