---
title: "Iterator protocol phase 4 — complete container coverage; array is not protocol-iterable (#1382)"
status: Accepted
issue: 1382
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Iterator protocol phase 4 (#1382)

**Status:** Accepted — the closeout phase of the `Iterator[T]` epic (RFC
Discussion #1417). Phases 1–3 shipped the interfaces
(`decisions/lang/1382-iterator-protocol.md`), the `iter()` / `next()` built-ins
(`1382-iter-next-builtins.md`), and reified per-container iterators
(`1382-phase3-reified-iterators.md`). This phase adds no new behavior; it
completes the test coverage and the SPEC/STABILITY documentation, and records one
clarification.

## Completed coverage

Exhaustive reified-iterator goldens now span **every** built-in iterable
container, with the iteration order each already uses for `for x in c`:

- sequences — `list` / `str` (→ `char`) / `bytes` (→ `int64`) / `range` (→
  `int64`): in order, against the live length (phase-3 golden);
- `deque` — left-to-right; `oset` — insertion order; `tuple[T]` — construction
  order; `heap` — priority (min-first) order (this phase's golden);
- `dict` (keys) / `set`: unspecified order (D-1066), so goldens use a single
  element or an order-independent reduction.

`next(iter(c))` is the exact element type for each, enforced by sema tests.

## Clarification — `array` is not protocol-iterable

An `array` (the numeric N-D array, #24/D-0011) is **not** iterated through the
D-0048 protocol: it has no `for x in array` lowering and no comprehension
`iterate` arm. Arrays are consumed by **indexing, slicing, and vectorized
operations**, not element-by-element iteration. So `iter(array)` is **not**
supported (phase 3's SPEC note that listed `array` among the iterable containers
was inaccurate and is corrected here). This is deliberate: element iteration of a
numeric array would defeat its vectorized model; a program that wants per-element
iteration converts to a `list` first.

## No behavior change

The `iter()` / `next()` / `map()` / `filter()` implementation is unchanged from
phases 2–3; this phase is goldens, sema tests, SPEC, and STABILITY only. With it,
#1382 and its epic are complete.
