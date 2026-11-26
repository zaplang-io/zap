---
title: "E6: serde breadth slice 1 — float64 + bytes fields"
status: Accepted
issue: 1574
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# serde breadth slice 1 — `float64` + `bytes` (#1574)

**Status:** Accepted — the first breadth slice for the bundled `serde` foundation
(#1570), extending it beyond the initial `int64` / `str` / `bool` + nested-struct
surface to cover **`float64`** and **`bytes`** scalar fields (encode + decode).

## Change

- `serde.Serializer` gains `emit_float(float64)` and `emit_bytes(bytes)`;
  `serde.Deserializer` gains `read_float(str) -> float64` and
  `read_bytes(str) -> bytes`.
- The generic `serialize` / `deserialize` drivers gain `float64` and `bytes`
  branches (dispatched on `f.ty.name`), alongside the existing kinds. No change to
  the monomorphization model — the new paths lower to the same direct `fieldget` /
  `fieldset` + direct backend calls.

## The bundled-interface ripple (a deliberate, recorded consequence)

Because `serde` is bundled in the compiler (Option B, #1570), **extending the
backend interfaces is a breaking change for every implementor on the next
`ghcr.io/zaplang-io/zap:latest`**: a backend that lacks the new methods no longer
satisfies the `[S: Serializer]` / `[D: Deserializer]` bound and fails to compile.
This is the flip side of the version-coherence the bundled model buys (one blessed
serde, but the whole ecosystem evolves with it).

Consequences, handled:
- In-repo backends are updated in lockstep: `tests/golden/serde_bundled_import.zap`
  and the bundled host test's inline backend gain the four methods.
- The **self-contained goldens** (`serde_core_roundtrip`, `serde_binary_backend`)
  declare their *own* interfaces and do not `import serde`, so they are unaffected
  and are left unchanged as original-surface proofs.
- **`zaplang-io/msgpack`** must add the four methods to `MsgpackSerializer` /
  `MsgpackDeserializer` — a coordinated follow-up PR there, gated on this landing +
  the `:latest` image rebuild (the same pattern as the E6.3 integration #24/#25).

This argues for stabilizing the interface in as few bumps as possible: this slice
adds the two concrete scalar gaps; `Optional` and collections (which need
`emit_nil` / sequence / map methods) are the next bump.

## Proof

`tests/golden/serde_bundled_import.zap` gains a `Measure{int64; float64; bytes}`
struct that serializes through the real bundled `serde` and round-trips every
field (float and bytes included); the encoded token stream is asserted exactly.
Runtime is gated by the CI LLVM-18 matrix; the host test
`the_serde_core_is_a_bundled_pure_zap_stdlib_module` keeps asserting cross-module
monomorphization with the widened interface.

## Deferred

`Optional`, `list[T]`, `dict[str, V]`, the integer-width tower, and enum (decode
blocked on #1561) are later slices. A direct DOM-free `MsgpackDeserializer` (to
beat the hand-rolled decode measured for E6.3) is a msgpack-side follow-up.
