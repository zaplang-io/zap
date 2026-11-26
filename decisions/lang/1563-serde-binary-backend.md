---
title: "E6.2: a real binary format backend on the serde core"
status: Accepted
issue: 1563
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# A real binary format backend validates the serde visitor model (#1563)

**Status:** Accepted — the second E6 (#1552) slice. Where #1562 shipped the serde
core with a trivial token backend, this slice proves the visitor model against a
**real, byte-checkable binary format** implemented purely as a `Serializer` /
`Deserializer` backend — no compiler change, no per-codec special-casing. This is
the concrete demonstration of the #1248 thesis: a new codec is a pure library.

## What is added

A compact self-describing **TLV binary codec** (`BinWriter` / `BinReader`) and a
golden exercising it through the shared generic `serialize` / `deserialize` free
functions (#1562). Wire format:

| kind         | bytes                                             |
|--------------|---------------------------------------------------|
| begin / end  | `0x10` / `0x11`                                   |
| `int64`      | `0x01` + 8 bytes big-endian (two's complement)    |
| `str`        | `0x02` + 1-byte length + UTF-8 bytes              |
| `bool`       | `0x03` + 1 byte (`0`/`1`)                         |

Field names are not written (positional; the decoder walks `reflect` fields in
declaration order). `BinWriter` accumulates a `list[int64]` and `bytes(buf)`s it;
`BinReader` reads from `bytes` with a cursor. Both are ordinary Zap structs that
structurally satisfy the backend interfaces — the `[S: Serializer]` /
`[D: Deserializer]` bound dispatches them statically (monomorphized, unboxed).

## Why a binary backend (not JSON)

The E6 payoff is binary serialization (msgpack, E6.3). A binary backend is the
distinct, load-bearing validation that the visitor model carries real wire bytes
(byte-order, length prefixes, nesting frames) — not just a text round-trip — and
is self-contained in the zap repo (no external package). It is deliberately not
the msgpack format itself: msgpack is the E6.3 non-core package.

## Proof (CI-LLVM-gated + box-verified)

`tests/golden/serde_binary_backend.zap` asserts, every line `True`: a flat struct
serializes to the **exact** wire bytes and round-trips; edge scalars (negative
int two's-complement, empty string, `False`) round-trip; a multi-byte UTF-8 string
round-trips (length counted in bytes); and a nested struct field encodes as nested
frames **byte-exactly**. Runtime agreement is gated by the **CI LLVM-18 matrix**;
it was also verified on the x86 LLVM-18 box ahead of CI (all five `True`). A host
test (`a_binary_codec_is_a_pure_serde_backend_with_no_compiler_change`) asserts the
lowering specializes `@serialize$T$BinWriter` / `@deserialize$T$BinReader` per
(type, backend) and calls backend methods directly — runnable without LLVM.

## Scope boundary and the upcoming packaging decision

- Nested-struct **decode** is encode-only here (same boundary as #1562): the free-
  function `deserialize` cannot name a nested field's type as a type argument from
  reflection, and enum/generic construction is blocked on #1561. Flat decode only.
- **The serde core is re-declared in this golden** because it ships as a self-
  contained proof, not an importable module. This is fine in-repo, but **E6.3**
  (the msgpack package, a separate repo) must `import` the core — so packaging the
  serde core as an importable module/package is the next required decision, and it
  is deferred to the maintainer. No new core primitive was added by this slice.
