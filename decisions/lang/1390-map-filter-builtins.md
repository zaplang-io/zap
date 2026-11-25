---
title: "map() / filter() built-ins (materialized list adapters) (#1390)"
status: Accepted
issue: 1390
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `map()` / `filter()` built-ins (#1390)

**Status:** Accepted — the `map` / `filter` built-ins, part of the Python-builtin
parity epic (#1387). Builds on first-class function values + closures (#163). Two
design points the issue flagged are decided here.

## What ships

- **`map(func, *lists)`** applies `func` across one or more lists, **stopping at
  the shortest** (the `zip` rule), producing a fresh **`list[R]`** where `R` is
  `func`'s return type.
- **`filter(pred, list)`** produces a fresh **`list[T]`** of the elements for
  which `pred` returns truthy, **preserving** the element type `T`.

```zap
list[int64] ys = map(square, [1, 2, 3])          # [1, 4, 9]
list[int64] zs = map(add, xs, ys)                # N lists, zipped and applied
list[int64] ev = filter(is_even, [1, 2, 3, 4])   # [2, 4]
print(sum(map(square, xs)))                      # composes with sum()/for/list()
```

## Decision 1 — materialized, not strictly lazy (for now)

The issue frames `map`/`filter` as **lazy** iterator adapters sharing the
`enumerate`/`zip`/`reversed` machinery. The **shipped** reality of those siblings
is that they are **materialized**, not lazy: `enumerate`/`zip` are typed as
`list[(…)]` (and only usable in a `for` header today), and `reversed` returns a
**real `list` value** (`zap_list_copy` + `zap_list_reverse`).

`map`/`filter` follow **`reversed`'s materialized-list model**: each produces a
real `list` value. Rationale:

- **Consistency** with the actual shipped adapters (no new lazy-iterator
  machinery that `enumerate`/`zip` themselves do not yet have).
- **Composition for free**: because the result is a `list`, `for y in map(…)`,
  `list(map(…))`, and `sum(map(…))` all work through the existing `list`
  consumers, and the result binds to a `list[...]` annotation directly.
- **Host- and CI-testable** end to end without new iterator-adapter plumbing.

**Strict laziness** (no intermediate list, so `sum(map(f, xs))` allocates nothing)
is a **follow-up**, best done as one cross-cutting change that makes *all* the
adapters (`enumerate`/`zip`/`map`/`filter`) real lazy `Iterator[T]` values at once
— aligned with #1382 phase 3 (reified iterators). Deferring it keeps this PR
consistent with today's codebase rather than making `map`/`filter` the lone lazy
outliers.

## Decision 2 — `filter(None, xs)` deferred

Python's `filter(None, xs)` (keep the truthy elements) needs a null-predicate
overload, which the issue flagged as a design decision. It is **deferred**: a
clean compile error (`ZAP4024`) points at the follow-up rather than silently
accepting or guessing a spelling. The recommendation for the follow-up is to
support `filter(None, xs)` exactly as Python does (special-case the `None`
literal in the first argument to a truthiness filter), pending maintainer
confirmation of the spelling.

## Other scope

- **Lists only** for now (the `enumerate`/`zip` precedent); other iterables are a
  follow-up.
- `map`'s result element type is the callee's return type — resolved precisely for
  a **named function** (looked up in the signature table), and left loose
  (`Unknown`, precise at lowering) for a closure / method value. Closure and
  method-value callees work through the first-class-function-value path (#163);
  precise sema typing of their result element is a follow-up.

## Implementation

- **sema** (`crates/sema/src/lib.rs`): `map` / `filter` join `BUILTINS`;
  `infer_call` arms type `map` as `list[R]` (named-callee return type, else
  `Unknown`) and `filter` as `list[T]` (input element type), validating list
  iterables, arity (`ZAP4025`), and the deferred `filter(None, …)` (`ZAP4024`).
- **IR lowering** (`crates/ir/src/lower.rs`): `lower_map_builtin` computes the
  shortest length across the lists, then an index loop reads one element from each
  list (coerced to the matching `func` parameter type), calls the function value
  (`emit_closure_call`), and pushes the result into a fresh `list[R]`.
  `lower_filter_builtin` loops the single list, calls the predicate, and pushes
  the element when the result is truthy. Both reuse the existing list-build
  helpers (`zap_list_new` / `zap_list_get_*` / `zap_list_push_*`); no new runtime.

## Tests

- Host (sema): `map_result_is_a_list_of_the_callee_return_type`,
  `map_over_multiple_lists_types_by_the_callee`, `filter_preserves_the_element_type`,
  `filter_none_is_a_deferred_follow_up`, `map_filter_arity_is_rejected`; plus the
  `map_filter_builtin_codes` code-pin in `error_codes.rs`.
- Runtime golden (CI LLVM matrix): `tests/golden/map_filter_builtin.zap` — `map`
  over one and two lists (stop-at-shortest), `filter` by a predicate, the
  `sum(map(…))` / `for … in filter(…)` / `filter(…, map(…))` compositions, and
  empty inputs.

## Follow-ups

Strict laziness across all adapters (with #1382 phase 3), `filter(None, xs)`,
closure / method-value result typing, and other-iterable inputs.
