---
title: Variadic tuple[T] — hashing & dict/set keys (Phase 2, part 2)
status: Locked
issue: 1474
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `tuple[T]` hashing / dict-set keys

**Status:** Locked. Part 2 of #1474 (epic #1461), completing it; equality (part 1,
`decisions/lang/1474-vartuple-equality.md`) is the prerequisite.

## Decision

A `tuple[T]` is **hashable** — usable as a `dict` key or `set` member — when its
element kind is directly hashable: an `int*` / `bool` / `char` / tier-1 enum
(word), `f64`, or `str` / `bytes`. The hash is the ordered fold of the element
hashes (via `zap_hash_combine`, the same fold a fixed tuple uses), so two tuples
equal under the element-wise `==` hash equal (`a == b ⟹ hash(a) == hash(b)`).

- **Not yet hashable:** a `tuple[T]` whose element is itself an aggregate (a
  nested `tuple`, `list`, struct, complex, …). Sema reports it as not hashable,
  matching the lowering, rather than over-accepting. A recursive hash is a
  follow-up.

## Representation

The key path selects a key's storage/hash function from its `Elem::key_suffix()`,
and a `tuple[T]` element previously collapsed to `Elem::Nested` (pointer-key path
— wrong, it would hash the buffer's identity). This phase adds a dedicated
**`Elem::VarTuple(u32)`** kind:

- physically a pointer word like `Elem::Nested` (same `_ptr` storage, GC tracing,
  and the same lowering interner `u32` recovering the full `Ty::VarTuple(...)`
  for read-back — so iterating keys/members yields a `tuple[T]`, not a bare ptr);
- but `key_suffix() == "fn"`, routing to the **content-hash** key path.

`key_fn_args` synthesizes, per element-kind tag, a one-call `(__hash__, __eq__)`
wrapper pair — `$vt$<tag>$hash(ptr) -> i64` / `$vt$<tag>$eq(ptr, ptr) -> i1` —
over the runtime `zap_vartuple_hash` / `zap_vartuple_eq`, memoized module-wide,
and hands their addresses to the dict/set as the key functions (exactly the
mechanism a fixed-tuple / struct key uses).

## Consequences

- New `Elem::VarTuple` variant (ABI-identical to `Elem::Nested`: a traced
  pointer, one codegen `llvm_type` grouping) and runtime `zap_vartuple_hash`.
  `tuple[T]` is now admitted at `dict`-key and `set`-member positions by
  `ty_auto_hashable`.
- Covered by golden `vartuple_key` (tuple keys in a dict and a set, content
  dedup, str-element keys); the typed constructors are Zap-only so it is golden-
  only (not a CPython differential). Completes #1474; ordering/concat/slicing is
  #1475, exhaustive goldens + SPEC is #1476.
