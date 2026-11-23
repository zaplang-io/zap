---
title: Iterable min/max (key=/default=) and sum start
status: Locked
issue: 1299
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# Iterable `min`/`max` (`key=` / `default=`) and `sum(xs, start)`

**Status:** Locked

## Decision

Complete the reduction built-ins (epic #1286), following the scalar/variadic
`min`/`max` (#1279/#1284) and element-typed `sum` (#1284):

- **`min(xs)` / `max(xs)`** over a `list[T]` return the lesser / greater
  **element** (type `T`). The lowering drains the list once, seeding the best
  element from element 0 and folding a compare over the rest. Elements of an
  orderable kind compare directly: integers/bools/chars/enums with a signed
  `icmp` (never through float), floats with `fcmp`, and `str`/`bytes` with
  `zap_str_cmp`. A tie keeps the earlier element (Python semantics). An empty
  list raises a catchable **ValueError** unless `default=` is given.
- **`min(xs, key=f)` / `max(xs, key=f)`** project each element through a function
  value `f: fn(T) -> K` (an orderable key `K`) and keep the element whose key is
  best — the element is returned, not the key. The best element and its key live
  in stack slots so they stay GC-reachable across the key call's safepoint (the
  same technique as `sorted`'s `build_key_list`).
- **`min(xs, default=d)` / `max(xs, default=d)`** return `d` for an empty list
  instead of raising; `d` must be assignable to the element type.
- **`sum(xs, start)`** seeds the accumulator with `start` (coerced to the element
  type) instead of `0`, so `sum(xs, start) == start + sum(xs)`, staying exact for
  integers. `start` must be numeric.

The single-positional / variadic disambiguation matches Python: **one** positional
argument is the iterable form (`min(xs)`), **two or more** are the variadic
scalar form (`min(a, b, …)`, #1284). `key=`/`default=` apply only to the iterable
form (ZAP4025 on the variadic form); an unknown keyword is ZAP4023; a
non-orderable element without `key=` is ZAP4024.

### Deferred

- Non-list iterables (generators) in the iterable `min`/`max` form, and `start=`
  there, stay the general-iterator follow-up; `sum` already drains any iterator,
  so `sum(xs, start)` works for generators too.
- `str`/`bigint`/`complex` element support beyond the orderable kinds above.

## Rationale

- **Reuse proven loop shapes.** The reduction loop mirrors `build_key_list`
  (index loop, per-element key call, GC-safe accumulator in a slot) and the
  empty-sequence raise mirrors `range()`'s zero-step `ValueError`. `key=` reuses
  `emit_closure_call`; element ordering reuses the compare instructions the
  scalar `min`/`max` already emit, plus `zap_str_cmp` for strings.
- **Return the element, compare the key.** Keeping the best element and its
  projected key in parallel slots is the minimal state for `key=`, and keeps the
  integer-exactness guarantee for integer keys.
- **`start` is positional**, matching Python's `sum(iterable, start)`, so no
  keyword and no `seam_sigs.rs` change.

## Consequences

- `min(list)`/`max(list)` (with `key=`/`default=`) and `sum(xs, start)` behave as
  Python's, exact for integers (golden `iter_reduce`).
- An empty `min(xs)`/`max(xs)` without `default=` raises a catchable ValueError.
- Part of epic #1286. Remaining follow-ups: general (generator) iterables for
  `min`/`max`, and the `enumerate`/`zip` for-loop built-ins (#1302, shipped in
  the same change).
