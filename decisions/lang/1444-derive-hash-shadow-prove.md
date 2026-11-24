---
title: "E4 shadow-proves @derive(Hash) over the @derivable mechanism"
status: Accepted
issue: 1444
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Shadow-prove `@derive(Hash)` (#1444)

**Status:** Accepted — an E4 (#1429) dogfood slice, same shape and rationale as
the Eq shadow proof (see `decisions/lang/1431-derive-eq-shadow-prove.md`): a
library `@derivable` generator expresses the derive and a behavior-identical
golden shows it agrees with the built-in, **changing no wiring**.

## Built-in

`@derive(Hash)` → `gen_hash_method` → `__hash__(self) -> int64`: seed `acc = 0`,
then for each field in declaration order `acc = zap_hash_combine(acc,
emit_elem_hash(field))`.

## Shadow — byte-identical

A library `@derivable(StructHash)` generator: `int64 h = 0; comptime for f in …
fields: h = hash_combine(h, reflect.field(value, i).__hash__())`. The source
`hash_combine` lowers to `zap_hash_combine`, and the `.__hash__()` capability
dunder lowers to the **same** per-field hash as `emit_elem_hash` (I#820). With the
same seed (0) and the same left fold in declaration order, the shadow produces the
**same hash value** as the built-in — confirmed in IR (both `iconst 0` →
`zap_i64_hash` / `zap_str_hash` → `zap_hash_combine`).

## What the golden asserts

Raw hash values are wyhash-seed-dependent per run, so the golden asserts built-in
`== shadow` hash **equality** (`p.__hash__() == p.structhash()`) and
equal-structs-hash-equal consistency — never a fixed hash number.

## Guard

- Golden `tests/golden/derive_hash_shadow.zap` (runtime output, CI LLVM matrix).
- Host test `a_library_generator_shadows_the_built_in_hash_without_replacing_it`:
  both `@P____hash__` (built-in, untouched) and `@_derive_structhash$P` emitted,
  the shadow folding via `zap_hash_combine`.

No wiring changes; the built-in `gen_hash_method` (and the value-struct
auto-derive of Eq+Hash) stay.
