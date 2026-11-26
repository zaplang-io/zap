---
title: "E6: bundled pure-Zap stdlib modules; serde ships blessed in-language"
status: Accepted
issue: 1570
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Bundled pure-Zap stdlib modules; `serde` is the first (#1570)

**Status:** Accepted — a D-0081 (module resolution) extension that lets the
compiler ship **pure-Zap stdlib modules** bundled in the toolchain, resolved by
`import <name>` with no `zap add`. The first is `serde`, the serialization
foundation (E6, epic #1552 / #1248).

## The decision: serde is blessed in-language (Option B), not a standalone package

A `Serialize` / `Deserialize` foundation is **near-universal fan-in** across the
`zaplang-io` ecosystem — every codec (json/yaml/csv/msgpack/avro/toml/cbor/
protobuf), every API & cloud client (aws, azure, gcp, kubernetes, grpc, graphql,
elasticsearch, requests, llm), the wire-protocol clients (redis, mongodb, kafka,
nats, …), observability (otel, prometheus, sentry) and auth (jwt) are all serde
consumers. Shipping it as one external package (the Rust `serde` model) would
leave that whole graph exposed to **version skew**; a blessed in-language module
gives exactly one version, always present. That is why E6 puts serde in the
language rather than in its own package repo.

But there was **no mechanism** to ship a pure-Zap stdlib module: the existing
stdlib modules are Rust *seams* (`crates/sema/src/modules/`), which cannot host
interfaces, generics or `comptime` reflection — exactly what serde is — and there
is no embedded `.zap` prelude. This change adds that mechanism.

## The mechanism (resolution tier 1b)

- **Registry** `crates/driver/src/bundled_stdlib.rs`: maps a module name to
  embedded pure-Zap source (`include_str!` of `crates/driver/stdlib_zap/<name>.zap`),
  with a synthetic path label (`<stdlib-zap:<name>>`) used as the linker's dedup
  key and diagnostic file label.
- **Linker** `crates/driver/src/modlink.rs`: a new resolution tier **1b**, after
  the Rust-seam check (`is_builtin_module`) and **before** dependency packages —
  so a bundled name is never shadowed by a same-named `zenv` package. A bare
  `import <name>` for a bundled module resolves to the synthetic label;
  `load_file` recognizes it and sources the embedded text instead of the
  filesystem. The module is then parsed and **merged like any other module** (its
  own mangle prefix, qualified `serde.foo` access), so it carries interfaces,
  generics and comptime reflection — a dependency-module merge in all but origin.
- Bare `import <name>` only; a dotted `import <name>.sub` falls through to the
  dependency / local tiers (a bundled module is a single file today).
- The mechanism is **reusable** for any future pure-Zap stdlib module.

`design/0034-modules.md` §2 (path resolution) records tier 1b alongside the
existing 1/2/3.

## `serde` as the first bundled module

`crates/driver/stdlib_zap/serde.zap` ships the serde core proven in #1562/#1563:
the `Serializer` / `Deserializer` backend interfaces and the generic
`serialize[T, S: Serializer]` / `deserialize[T, D: Deserializer]` drivers (scalar
`int64`/`str`/`bool` + nested-struct encode, flat decode). A consumer does
`import serde`, implements the backend interfaces on a local struct, and calls
`serde.serialize(value, backend)` / `serde.deserialize[T, Backend](d)` — **fully
monomorphized across the module boundary** (the lowered ZIR shows
`@…serde__serialize$Point$Backend`, direct backend calls, no vtable), with no
compiler change per codec. This is the #1248 thesis delivered as a usable,
blessed foundation.

## Proof

- Golden `tests/golden/serde_bundled_import.zap` (+ `.expected`, 3× `True`): a
  user program `import serde`s, serializes/round-trips its own struct through a
  local backend, and nested-encodes — runtime gated by the **CI LLVM-18 matrix**.
  (The merge produces the same shape a dependency module would, which the tier-2
  runtime path already exercises, and the serde logic is identical to the
  box-verified #1562 core.)
- Host test `the_serde_core_is_a_bundled_pure_zap_stdlib_module` asserts `import
  serde` resolves from the registry and the drivers specialize across the module
  boundary — runnable without LLVM. Plus `bundled_stdlib` Rust unit tests.

## Scope boundary

Enum / generic-struct **decode** stays blocked on the `reflect.construct`
generalization (#1561). Collection / `Optional` field breadth is a follow-up. No
new core comptime/reflection primitive was added — only the module-delivery
mechanism. **Unblocks E6.3** (zaplang-io/msgpack#24): msgpack can now `import serde`.
