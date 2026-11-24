---
title: Nameable Iterator[T] / Iterable[T] interfaces and the iter/next built-ins
status: Locked
issue: 1382
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# `Iterator[T]` / `Iterable[T]` + `iter()` / `next()`

**Status:** Locked (surface). Implemented in phases — see "Phasing".

Full rationale and the decision debate: RFC discussion
[zaplang-io/zap#1417](https://github.com/zaplang-io/zap/discussions/1417).

## Decision

Introduce precise, **nameable** iteration types and the explicit iteration
built-ins, so library code can accept/return/step iterators by a stable type.
The fast index-based `for` loops are unchanged; this is an additive, opt-in
reified surface over the existing D-0048 protocol.

1. **Names:** `Iterator[T]` and `Iterable[T]` (Python-style).
2. **Kind:** structural **interfaces** (generic, one type parameter `T`), not
   concrete types — the representation stays free to evolve behind them.
   - `Iterable[T]` requires `__iter__(self) -> Iterator`.
   - `Iterator[T]` requires the comma-ok `__next__(self) -> (T, bool)` (the
     D-0048 primary form; the hot iteration path never raises). `Iterator` is
     **not** a structural refinement of `Iterable` — a generator expression has
     `__next__` but no `__iter__` yet is a valid iterator; "an iterator is
     itself iterable" is realized by `iter(it)` returning it, not by requiring
     every iterator to define `__iter__`.
3. **`iter(it)` idempotent:** returns the same iterator for something already an
   iterator.
4. **`next` contract:** `next(it)` raises catchable `StopIteration` at
   exhaustion; `next(it, default)` returns `default`. Both are built on the
   comma-ok `(T, bool)` internally, so `for` loops and the `default` form never
   raise.
5. **Representation:** `iter()` yields a **typed synthesized generator** (reuses
   the free-generator machinery, which carries a precise `(T, bool)` — unlike
   the deliberately loose genexpr type), surfaced through the `Iterator[T]`
   interface.
6. **Element typing:** `next(iter(xs))` is statically the exact element type.
   This flows through the concrete iterator's own `__next__` signature, since
   `Ty::Named` carries no type arguments.
7. **Semantics:** single-pass / exhaustible; the iterator is a live **view**
   (container reference + cursor), not a snapshot; sequence containers
   (`list`/array/`str`/`bytes`/`range`) iterate against the **live length** each
   step (matching the current `lower_for_list`); `dict`/`set` mutation during
   iteration is **undefined** in v1 (Go's stance; a Python-style `RuntimeError`
   modification-guard is a follow-up); iterators are single-owner / not
   coroutine-safe (the `--race` detector catches misuse).
8. **`Iterable[T]` conformance:** `list`/`set`/`dict`/`str`/`bytes`/`range`/
   array; `dict` iterates **keys** by default (matching `for`).

## Phasing

1. **`Iterator[T]` / `Iterable[T]` interface types + sema recognition** — this
   change. The two interfaces are registered as the first *generic*
   compiler-known interfaces (`register_builtin_iteration_interfaces`), and
   structural conformance at the element type works (`conforms_with_args`).
2. `iter()` / `next()` over the D-0048 protocol (generators/genexprs/user
   types), with `StopIteration` + `default`.
3. Precise reified per-container iterators (typed synthesized generators), each
   conforming to `Iterator[T]`.
4. Exhaustive goldens across every container kind + SPEC/STABILITY docs.

## Prerequisite (gating phases 2–3)

Enforcing `Iterator[T]` at **value positions** (a parameter/return/variable
typed `Iterator[int64]`) needs **generic-interface type arguments carried in the
type representation**: today a value-position annotation resolves to
`Ty::Named("Iterator")` with the `[int64]` dropped, and `coercible -> conforms`
checks the interface with no type arguments — so the element type is not
enforced there. This is a pre-existing limitation of **every** generic interface
(the #1205 family), not Iterator-specific, and its fix (e.g. a `Ty` that carries
interface type arguments, threaded through `coercible`/`conforms`/`subst_ty`) is
a separate core change this epic depends on.

## Consequences

- `Iterator` / `Iterable` are predefined interfaces available without
  declaration; a user interface of the same name still overrides (they are
  registered before user items, like the capability interfaces).
- `iter` / `next` are not yet built-ins (phase 2); container iterators are not
  yet reified (phase 3); value-position element-type enforcement awaits the
  prerequisite above.

## Out of scope

Async iteration (`aiter`/`anext`) — Zap is uncolored; `itertools`-style
combinators; sized/reversible refinements.
