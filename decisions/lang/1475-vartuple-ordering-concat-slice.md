---
title: Variadic tuple[T] — ordering, concatenation, slicing (Phase 3)
status: Locked
issue: 1475
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `tuple[T]` ordering / `+` / slicing

**Status:** Locked. Phase 3 of the variadic-tuple epic #1461. Completes the
sequence surface (after the type+core #1473 and equality+hashing #1474).

## Decision

- **Ordering** `< <= > >=`: lexicographic over two `tuple[T]` values — the first
  differing element decides, and on a common prefix the shorter tuple is less
  (CPython tuple semantics). Types as `bool`; ordering a `tuple[T]` against a
  non-tuple is rejected (as for lists). Available when the element kind is
  orderable (word / `str`).
- **Concatenation** `+`: `tuple[T] + tuple[T]` → a fresh `tuple[T]` with the
  left's elements then the right's.
- **Slicing** `t[a:b]` / `t[a:b:c]` (including negative indices and a negative
  step) → a fresh `tuple[T]`.

## Representation (reuse)

A `tuple[T]` is a frozen `ZapList` buffer, so all three reuse the list
machinery rather than new per-op kernels:

- ordering reuses `list_order_cmp` (the IR-synthesized lexicographic comparator),
  parameterized by the element kind;
- slicing reuses `zap_list_slice`, relabelling the fresh buffer as `tuple[T]`;
- concatenation is the one new runtime, `zap_vartuple_concat(a, b, kind)`, which
  copies both buffers' elements (by the element-kind tag 0=word/1=f64/2=str) into
  a fresh buffer — lists have no `+`, so there was nothing to reuse.

sema already typed container ordering as `bool` and list `+`/slice; the
`tuple[T]` arms extend `arith_result` (`+` → `tuple[T]`) and the slice inference
(`tuple[T]`), and ordering falls through to `bool` (not in the non-orderable set).

## Consequences

- One new runtime entry point (`zap_vartuple_concat`); ordering and slicing add
  no runtime. Covered by golden `vartuple_ops` and CPython differential
  `vartuple_ops`, plus the sema acceptance `vartuple_ops_codes`.
- The exhaustive per-element-kind goldens + SPEC/STABILITY are #1476; with this
  phase the `tuple[T]` surface (construct, index, iterate, len, repr, ==, hash/
  key, order, concat, slice) is feature-complete for v1.
