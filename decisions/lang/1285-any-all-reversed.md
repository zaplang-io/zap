---
title: any / all / reversed built-ins
status: Locked
issue: 1285
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# `any` / `all` / `reversed`

**Status:** Locked

## Decision

Add three idiomatic-loop built-in free functions (epic #1286):

- **`any(xs)` / `all(xs)`** over a `list[bool]` → `bool`. The lowering drains the
  list once (the proven `iterate_list` path), folding the `i1` accumulator with
  `or` (`any`) / `and` (`all`). The accumulator's initial value is the empty-list
  result, so `any([])` is `False` and `all([])` is `True` (matching Python). The
  drain is not short-circuited — for a finite list the result is identical.
- **`reversed(xs)`** → a **new** `list[T]` with `xs`'s elements in reverse order,
  leaving `xs` unchanged. It is a shallow copy (`zap_list_copy`) reversed in place
  (`zap_list_reverse`), so `for x in reversed(xs)` iterates the new list through
  the ordinary list path, and `reversed(xs)` is also usable as a value.

### Rejected / deferred

- `any`/`all`: non-`list[bool]` argument → ZAP4024 (non-bool elements and other
  iterables such as generators are follow-ups), wrong arity → ZAP4025, any
  keyword → ZAP4023.
- `reversed`: non-list argument → ZAP4024 (reversing a `str` or other iterable is
  a follow-up), wrong arity → ZAP4025.
- **`enumerate` and `zip` are a follow-up** (filed separately, attached to epic
  #1286): used idiomatically as `for i, x in enumerate(xs)` / `for a, b in
  zip(p, q)`, they need new `for`-loop desugaring (or first-class iterator
  values) with tuple-target binding — control-flow lowering best landed and
  gated on its own.

## Rationale

- **Reuse proven machinery.** `any`/`all` fold a pure closure over the same
  `iterate_list` drain that `sum` uses; `reversed` composes two existing list
  runtime ops (`copy` + `reverse`). No new loop structure or runtime function is
  introduced, so the change is mechanical and verifiable from the lowered IR.
- **`reversed` as a materialized list, not a lazy iterator.** A new reversed list
  gives identical observable behavior for `for x in reversed(xs)` and
  `list(reversed(xs))` and needs no iterator-protocol object; laziness is an
  optimization that can come later without changing semantics.
- **`list[bool]` first.** The dominant use is `any([pred for x in xs])` /
  `all(...)`, a `list[bool]` from a comprehension. Numeric/other-element and
  generator inputs (which Python also accepts) are a clean follow-up, and
  restricting now keeps the result unambiguous.
- **Defer `enumerate`/`zip`.** Those are the control-flow-heavy members of the
  set; splitting them into their own change keeps this slice a set of localized,
  low-risk additions.

## Consequences

- `any(list[bool])` / `all(list[bool])` return a `bool` (golden `iter_builtins`),
  with the documented empty-list results.
- `reversed(list[T])` yields a new `list[T]`; the source is unchanged.
- Part of epic #1286. Follow-ups: `enumerate`/`zip`; `any`/`all` over generators
  and non-bool elements (with short-circuit); `reversed` over `str`/other
  iterables; and the iterable `min`/`max`/`sum` forms already tracked in #1299.
