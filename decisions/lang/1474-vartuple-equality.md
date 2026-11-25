---
title: Variadic tuple[T] — equality (Phase 2, part 1)
status: Locked
issue: 1474
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `tuple[T]` equality (`==` / `!=`)

**Status:** Locked. Part 1 of #1474 (epic #1461, variadic tuple). This ships
value equality; **hashing / dict-set-key use is the remaining part of #1474**
(see "Deferred" below).

## Decision

`t1 == t2` / `t1 != t2` on two `tuple[T]` values is **element-wise structural
equality**: equal length, then every element equal (CPython tuple semantics).
Sema already types `==`/`!=` on any container as `bool`; lowering routes two
`VarTuple` operands to a runtime `zap_vartuple_eq(a, b, kind)` that walks the two
frozen list buffers by an element-kind tag:

- `kind 0` — a plain 64-bit word (`int*`, `bool`, `char`, enum);
- `kind 1` — `f64` (compared as doubles);
- `kind 2` — `str`/`bytes` (`ZapStr`, compared by content via `zap_str_eq`).

An element kind outside those (a nested collection, struct, complex, function)
is a clean lowering error, not a silent wrong answer.

## Deferred to the rest of #1474: hashing / dict-set key

Making a `tuple[T]` a `dict`/`set` **key** needs it to carry the content-hashing
"fn" key path (a synthesized `__hash__`/`__eq__` pair), but the key's storage
function is selected by `Elem::key_suffix()`, and a `tuple[T]` element currently
collapses to `Elem::Nested` — which cannot be distinguished from an unhashable
`list` at the `Elem` level, and routes to the pointer-key path. Giving it the
"fn" path cleanly needs a dedicated key representation (an `Elem::VarTuple`
variant threaded through the key sites + synthesized per-element-kind
`zap_vartuple_hash`/`eq` wrappers). That is a focused, codegen-touching change
tracked as the remaining part of #1474; equality here is its prerequisite and
stands on its own.

## Consequences

- New runtime `zap_vartuple_eq`; no sema change (container `==` already types to
  `bool`). Covered by golden `vartuple_eq` and CPython differential
  `vartuple_eq`.
- Ordering (`< <= > >=`), concatenation (`+`) and slicing are #1475; the exhaustive
  element-kind coverage + SPEC/STABILITY is #1476.
