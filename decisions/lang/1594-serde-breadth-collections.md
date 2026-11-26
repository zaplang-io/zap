---
title: "E6: serde breadth slice 2 — uint64, list, dict"
status: Accepted
issue: 1594
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# serde breadth slice 2 — `uint64` / `list[T]` / `dict[str, V]` (#1594)

**Status:** Accepted — the second serde breadth slice, unblocked by the
comptime-for dispatch fix (#1586/#1584 → #1592). Extends the bundled `serde`
foundation (#1570) to unsigned integers and scalar-element collections, both
directions.

## Change

- `serde.Serializer` gains `emit_uint(uint64)`, and sequence/map framing:
  `begin_seq` / `end_seq`, `begin_map` / `end_map`.
- `serde.Deserializer` gains `read_uint(str) -> uint64`, `begin_seq(str) -> int64`
  (returns the element count), `end_seq`, `begin_map(str) -> int64` (returns the
  entry count), `end_map`.
- The `serialize` / `deserialize` drivers gain `uint64`, `list[T]`, and
  `dict[str, V]` branches for scalar elements (`int64` / `uint64` / `float64` /
  `str` / `bool`). Encode iterates the collection and emits each element/pair
  between `begin_*` and `end_*`; decode asks the backend for the count, reads that
  many elements/pairs into a concrete `list[E]` / `dict[str, V]`, and writes the
  field with `reflect.set_field`. A length-prefixed backend buffers on encode (no
  count up front) and recovers the count on decode from `begin_seq` / `begin_map`.

This is the first breadth that dispatches over **nameless** field types; it is
only expressible because #1592 made `f.ty.name` total (empty for nameless kinds),
so the `f.ty.name`-then-`f.ty.kind.name` dispatch chain folds for a list/dict
field instead of corrupting the `comptime for` unroll.

## The bundled-interface ripple

Widening `Serializer` / `Deserializer` requires every backend to implement the new
methods on the next `zap:latest`. In-repo backends (the serde goldens + host
tests) are updated in lockstep; `zaplang-io/msgpack`'s `MsgpackSerializer` /
`MsgpackDeserializer` get a coordinated follow-up (`emit_uint`,
`begin_seq`/`end_seq` → array header, `begin_map`/`end_map` → map header, and the
decode counterparts over the `Value` DOM), gated on this landing + the image rebuild.

## Proof

`tests/golden/serde_bundled_import.zap` encodes a `{list[int64]; dict[str,int64];
uint64}` struct through the real bundled `serde` (asserted token stream) and
round-trips a `uint64`; the host test asserts the `Coll` specialization and direct
`begin_seq` / `begin_map` backend calls. Full collection round-trip (counts on
decode) is proven on the x86 LLVM box and by the msgpack serde test, whose backend
carries real array/map length headers. Runtime is CI-LLVM-gated.

## Deferred

`Optional` needs a multi-inner-type comptime formulation — a `match` inside a
*doubly*-nested comptime `if`, a narrower residual beyond the #1592 fix — and is a
later slice. Enum / generic-struct decode remain blocked on #1561.
