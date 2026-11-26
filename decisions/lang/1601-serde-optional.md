---
title: "E6: serde breadth slice 3 — Optional[T] fields"
status: Accepted
issue: 1601
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# serde breadth slice 3 — `Optional[T]` (#1601)

**Status:** Accepted. Adds `Optional[T]` (scalar inner) to the bundled `serde`
foundation (#1570), both directions.

## This was not a core gap — it was the wrong accessor

`Optional` was thought to be blocked on a comptime defect (a `match` in a
doubly-nested comptime `if`). It is not. The real cause: the driver used
`f.ty.elem` on an `Optional`, and an `Optional`'s `TypeInfo` has no `elem` — the
accessor *raises*, which (exactly like the pre-#1586 `.name` raise) makes the
surrounding comptime `if` condition unfoldable, so the whole `if` falls to a
runtime `if` and the `comptime for` loop variable leaks as `unknown variable 'f'`.
The correct accessor for an `Optional`'s inner type is **`f.ty.inner`** (`.elem`
is for `list`, `.key` / `.value` for `dict`). With `f.ty.inner`, `Optional`
dispatch plus `match` compiles and runs today — no comptime fix required. (The
#1586 fix made `.name` total; the container accessors still raise on the wrong
kind, so they must be used under the matching `f.ty.kind.name` guard, as the
driver does.)

## Change

- `serde.Serializer` += `emit_nil` (encode `None`; a `Some` emits the inner value).
- `serde.Deserializer` += `begin_optional(field) -> bool` — `True` and leave the
  value to read (a `Some`), or consume a nil and return `False` (a `None`).
- The `serialize` / `deserialize` drivers gain an `optional` branch guarded by
  `f.ty.kind.name == "optional"`, dispatching on `f.ty.inner.name` for scalar
  inners (`int64` / `uint64` / `float64` / `str` / `bool`). Encode matches the
  `Optional` and emits the inner value or `emit_nil`; decode asks `begin_optional`
  and builds `Optional[T].Some(...)` or `Optional[T].None`, then writes the field
  with `reflect.set_field`.

## Proof

`tests/golden/serde_bundled_import.zap` serializes `Opt{int64; Optional[int64]
Some; Optional[str] None}` through the real bundled `serde` (asserted token stream
`<,1,9,nil,>`) and decodes it back, matching `Some(9)` and `None`. Runtime is
CI-LLVM-gated; a host test keeps the backend conformance (the new `emit_nil` /
`begin_optional`). Widening the bundled interface ripples to backends;
`zaplang-io/msgpack` gets a matching update as a follow-up.

## Deferred

`Optional` of a nested struct / collection, and enum / generic-struct decode
(#1561), remain later.
