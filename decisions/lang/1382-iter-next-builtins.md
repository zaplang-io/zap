---
title: "iter() / next() over the D-0048 protocol (#1382 phase 2)"
status: Accepted
issue: 1382
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `iter()` / `next()` — explicit D-0048 iterator access (#1382 phase 2)

**Status:** Accepted — phase 2 of the `Iterator[T]` epic (RFC Discussion #1417,
decisions locked by the maintainer 2026-11-24). Phase 1 (PR #1427) registered the
built-in generic `Iterator[T]` / `Iterable[T]` interfaces; #1428 made
generic-interface conformance hold at value positions. This phase adds the
`iter()` / `next()` built-ins over the existing D-0048 iteration protocol.

## Scope (phase 2)

`iter()` / `next()` operate on values that **already satisfy the D-0048
protocol** — a generator, a generator expression, or a user type implementing
`__iter__` / `__next__`:

- **`iter(x)`** obtains an explicit iterator and is **idempotent** on an iterator
  (a type with `__next__` returns itself; an `Iterator[T]` stays `Iterator[T]`).
  An `Iterable` yields its `__iter__` result (`Iterable[T]` → `Iterator[T]`).
- **`next(it)`** steps the iterator; at exhaustion it raises a catchable
  `StopIteration`. **`next(it, default)`** returns `default` at exhaustion and
  never raises.

**Reified per-container iterators** (`iter([1, 2])`, `iter("ab")`, …) are **phase
3** — a clean compile error here (`ZAP4024`) pointing at phase 3, not a silent
gap; `for` already iterates every built-in container. Consuming an abstract
interface-typed `Iterator[T]` value by hand (dynamic dispatch through the fat
pointer) is likewise phase 3; sema still types `next()` on an `Iterator[T]`
parameter precisely (the element type), so library signatures type-check today.

## Decisions realized (from the RFC, locked 2026-11-24)

- **Idempotent `iter`** (decision 3): `iter(it)` returns the same iterator for
  something already an iterator — so `iter(gen())` is `gen()` itself, a live view.
- **`next` contract** (decision 4): `next(it)` raises `StopIteration`;
  `next(it, default)` returns `default`. Both are built on the comma-ok
  `__next__ -> (T, bool)` internally, so the iteration hot path stays
  exception-free — the `StopIteration` is **constructed only** on an explicit
  `next(it)` past the end with no default (lowered as `zap_panic_typed` with the
  `StopIteration` runtime id). For a Python-form `__next__ -> T` iterator (which
  raises `StopIteration` itself), the no-default call simply propagates the
  signal and the default form catches it with a landing pad.
- **Exact element typing** (decision 6): `next(it)` is statically the iterator's
  exact element type — `next_elem(__next__.ret)` for a user `__next__ ->
  (T, bool)` / `-> T`, and the `[T]` argument for an `Iterator[T]`-typed value. A
  generator stays element-loose at sema (`Unknown`, matching its synthetic
  `__next__ -> (Unknown, bool)`); lowering computes the precise element type.
- **Result type** (decision 5): `iter(x)`'s result is the iterator's **own
  concrete type** (a generator's `$gen:…`, a user struct, …), which both gives
  `next()` the exact element type and conforms to `Iterator[T]` structurally
  (#1428), so an `iter()` result flows into an `Iterator[T]` annotation.
- **Semantics** (decision 7): single-pass / exhaustible; the iterator is a live
  view (container reference + cursor), not a snapshot; `next(it, default)`
  evaluates `default` eagerly (it is a call argument), matching Python.

## Implementation

- **sema** (`crates/sema/src/lib.rs`): `iter` / `next` join `BUILTINS`;
  `infer_call` arms type them via two helpers — `iter_result_ty` (the iterator a
  value produces, or `None` → a `ZAP4024` naming phase 3 for a container) and
  `next_result_ty` (the element type, requiring a `__next__` / `Iterator[T]`,
  else `ZAP4024`). `iterable_elem` gains an `Iterator[T]` / `Iterable[T]` arm. The
  `default` must be assignable to the element type (`ZAP4024` otherwise);
  arity/kwargs are `ZAP4025`/`ZAP4023`.
- **IR lowering** (`crates/ir/src/lower/stmts.rs`): `lower_iter_builtin` reuses
  `setup_iter` (call `__iter__`, or identity for an iterator); `lower_next_builtin`
  reuses `next_form` to classify the iterator, then drives one step — the comma-ok
  form branches on the `ok` flag (fresh element, or default / raise
  `StopIteration`), the Python form propagates or catches `StopIteration` with a
  landing pad mirroring `drive_iteration`. The dispatch is a two-arm addition to
  the built-in call chain in `lower.rs`.

## Tests

- Host (sema, no LLVM): `iter_next_over_user_iterator_types_precisely`,
  `next_element_type_is_enforced`, `iter_next_over_a_generator`,
  `next_on_an_iterator_typed_parameter`, `iter_over_a_container_is_phase_3`,
  `next_on_a_non_iterator_is_rejected`, `iter_next_arity_and_kwargs_are_rejected`;
  plus the `iter_next_builtin_codes` code-pin in `error_codes.rs`.
- Runtime golden (CI LLVM matrix): `tests/golden/iter_next_builtin.zap` —
  generator and user-iterator stepping, the `default` form at exhaustion, an
  explicit `next()` raising `StopIteration` caught by `try/except`, and `iter()`
  idempotency as a live view.

## Follow-ups

- Phase 3: reified per-container iterators (a typed synthesized generator per
  `list` / `set` / `dict` / `str` / `bytes` / `range` / array), each conforming to
  `Iterator[T]`, so `iter()` works over containers and an abstract `Iterator[T]`
  value is hand-steppable.
- Phase 4: exhaustive goldens across every container kind; SPEC/STABILITY
  expansion.
