---
title: "reflect.construct zero-inits an enum field and builds a typed container"
status: Accepted
issue: 1608
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# `reflect.construct` for enum fields and containers (#1603 + #1608)

**Status:** Accepted — two `reflect.construct` widenings (epic #1248,
maintainer-assigned, one PR) that complete the **decode** side of a recursive
library codec, the last core gaps before the E5 Json retire (#1558). They extend
the #1454 / #1490 / #1561 `reflect.construct` family.

## #1603 — zero-init an enum-typed field

`reflect.construct(T)` zero-constructs a struct by zero-initializing each field,
but an enum field had no zero (`no zero-initializer for type 'Flavor'`), so a
struct with an enum field (e.g. `json_aws_open_enum`'s `Scoop { Flavor flavor }`)
could not be decoded. `zero_expr` now zero-inits an enum field to the enum's
**first variant** (declaration order): the bare `Enum.Variant` for a payload-free
first variant, or `Enum.Variant(zero(payload…))` for a payload-carrying one, with
the same by-value cycle guard as a struct. An enum has no natural zero; the first
variant is a throwaway base the decoder overwrites via `set_field`, exactly as a
struct's zero fields are. (`zero_expr` gains an `enums` parameter.)

## #1608 — build a typed empty container

`reflect.construct(T)` now accepts a `list` / `dict` / `set` / `Optional` type,
not only a struct/enum. For a `list`/`dict`/`set` it emits the **typed empty
constructor** `list[E]()` / `dict[K, V]()` / `set[E]()` (via a new `type_to_expr`
renderer) rather than a bare `[]` literal — crucially, so `auto acc =
reflect.construct(E)` **pins the element type** and a later `acc.append(…)`
type-checks. A bare `[]` leaves the element type unbound, which defeated a
recursive nested-collection decoder. An `Optional` zero-inits to `None`.

## Why together, and why now

Both are `reflect.construct` zero-init widenings, and both are needed by the same
consumer: a library decoder that zero-constructs then populates. With #1604
(reflect folding in helpers → reflected type args), a decoder can now **recurse
into nested collections** (`list[list[T]]`, `dict[str, list[T]]`): build a typed
empty container (#1608) and append decoded elements, and construct a struct with
an enum field (#1603). This clears the two remaining core blockers for the E5
`@derive(Json)` retire (#1558).

## Tests

- `tests/golden/reflect_construct_enum_and_nested.zap` — a recursive library
  codec round-trips `list[list[int64]]` (encode→decode by index) and decodes a
  struct with an enum field (CI LLVM matrix).
- `crates/driver/tests/zap_cli.rs::reflect_construct_zero_inits_an_enum_field_and_builds_a_container`
  — both widenings compile (host, no LLVM).
