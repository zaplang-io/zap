---
title: enumerate and zip for-loop built-ins
status: Locked
issue: 1302
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# `enumerate` / `zip` (for-loop built-ins)

**Status:** Locked

## Decision

Add `enumerate` and `zip` as for-loop built-ins (epic #1286), the idiomatic-loop
members deferred from #1285:

- **`for i, x in enumerate(xs[, start])`** — iterate a `list[T]` by index,
  binding the running index (`start` + position, `start` defaults to `0`) and the
  element. A single-name target binds the `(int64, T)` pair as a tuple.
- **`for a, b in zip(p, q, …)`** — iterate up to the **shortest** input's length
  (Python's default `zip`, not `zip_longest`), binding one element from each list
  per iteration. The target is a tuple of N names matching the N iterables, or a
  single name binding the N-tuple.

Both are **special-cased in the for-loop lowering** (`lower_for`), like `range`,
so no intermediate list of tuples is materialized. Sema types `enumerate(xs)` as
`list[(int64, T)]` and `zip(a, b)` as `list[(A, B)]` so the for-loop's existing
tuple-target binding type-checks; the lowering never builds that list.

### Rejected / deferred

- A **standalone** `enumerate`/`zip` value (e.g. `list(enumerate(xs))`, or binding
  one to a variable) is a compile error (ZAP6001) pointing at the for-loop form —
  a first-class lazy iterator value is a follow-up.
- Non-list iterables are rejected (ZAP4024); other arities/keywords are ZAP4025.

## Rationale

- **For-loop desugaring, not iterator objects.** The dominant use of both is
  `for … in …`, and desugaring there (mirroring `range` and `lower_for_list`)
  reuses `zap_list_len`/`zap_list_get_*` and the tuple-target binding, with no new
  iterator-protocol type and no allocation per element. A lazy value form can be
  added later without changing this.
- **`zip` stops at the shortest.** Matching Python's default avoids a silent
  out-of-bounds on the longer inputs; the minimum length is computed once up
  front with a compare + `select` fold.
- **Type as `list[tuple]` for binding only.** Giving the call a `list[(…)]` type
  lets the unchanged for-loop tuple-target checker bind `i, x` / `a, b` precisely,
  while the lowering intercepts the loop so the list is never created.

## Consequences

- `for i, x in enumerate(xs)` (with optional `start`) and `for a, b in zip(p, q)`
  type-check and iterate correctly (golden `iter_reduce`).
- A standalone `enumerate`/`zip` value is a clear compile error, not a silent
  half-feature.
- Part of epic #1286, which this and #1299 complete. Follow-ups: standalone
  iterator-value forms and non-list iterables.
