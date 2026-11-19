---
title: list.pop plus index / reverse / copy — the remaining Python list methods
status: Locked
issue: 1095
pr:
date: 2026-11-19
domain: stdlib
supersedes:
superseded-by:
---

# `list.pop` plus `index` / `reverse` / `copy` — the remaining Python `list` methods

**Status:** Locked

## Decision

Add the built-in generic `list[T]` methods still missing against CPython's
`list` surface, closing #1095 (`pop`) and the Part-2 gap sweep in one change:

- **`pop()` → `T`** — remove and return the **last** element, O(1) (no shift).
- **`pop(i)` → `T`** — remove and return the element at index `i`, ordinary list
  indexing including **negative** indices; the non-last case preserves the order
  of the remaining entries. An **empty list** or an **out-of-range** index raises
  a catchable **`IndexError`** (CPython parity).
- **`index(value[, start[, stop]])` → `int64`** — the position of the first
  element equal to `value` within `[start, stop)`; raises a catchable
  **`ValueError`** when none matches (CPython raises `ValueError`, **not**
  `IndexError`). `start`/`stop` are optional, clamped the CPython slice way
  (negatives count from the end, both clamp to `[0, len]`).
- **`reverse()`** — reverse the elements **in place**.
- **`copy()` → `list[T]`** — a **shallow** copy: a new list with the same element
  words, so membership is independent of the original but reference elements are
  shared.

Equality for `index` routes through the same `binop(Eq, …)` path as `count` /
`remove` / `in`, so it matches `==` semantics for every element kind (numeric
joins, `str` by contents, struct `__eq__`). Element kinds — `int`, `float`,
`bytes`/`str`, and reference/struct pointers — are all preserved: `pop` reads the
element back exactly as a subscript read `xs[i]` does (its full type, #625), and
`copy` reuses `zap_list_copy` (also used by `sorted()`), which inherits the
list's `elem_ptr` kind so the precise collector traces the copy correctly.

The complete built-in `list` method set is now: `append`, `sort`, `extend`,
`insert`, `remove`, `pop`, `index`, `count`, `reverse`, `copy`, `clear`.

**Arguments stay positional (no keyword form).** `pop(i)` and
`index(value, start, stop)` are **positional-only**, matching CPython (which
rejects `l.pop(index=…)` / `l.index(x, start=…)` with a `TypeError`) and every
other built-in `list`/`deque` method. Built-in generic collection methods are
hand-written check/lower dispatch in `zapc_sema` / `zapc_ir`, **not** seam module
functions, so they do not appear in `crates/syntax/src/seam_sigs.rs` (which keys
on a module name such as `zlib`/`http`) and do not go through the keyword binder.
Consequently no `seam_sigs.rs` row and no `seam_kwargs.rs` case applies; the
positional-vs-keyword recommendation is **positional**, recorded here per the
workspace stdlib rule.

## Rationale

- **O(1) stack pop is the headline.** The native Havlak port
  (zaplang-io/zap-benchmarks#16) uses a `list` as an explicit stack of DFS
  frames and needs last-element pop with no equality search — `remove(x)` (a
  linear scan) is the wrong tool. `pop()` with no argument shifts nothing, so
  the hot path is constant-time.
- **Faults are raised the catchable way.** The `pop` runtime helper raises
  `IndexError` via `zap_panic_typed`, and the lowering reaches it through
  `throwing_call` (an `invoke` inside a `try`), so `try/except IndexError`
  catches it — the same discipline `remove`'s `ValueError` and a subscript read's
  `IndexError` already follow. A plain `emit(Call)` would terminate instead of
  unwinding to the handler.
- **One runtime word-mover per op, typed wrappers for read-back.** `pop` is a
  single `zap_list_pop_word` over uniform 8-byte words (element-type-agnostic,
  a GC leaf) behind `_i64`/`_ptr`/`_f64` wrappers mirroring `zap_list_get_*`, so
  a pointer element round-trips as a pointer and a float as a float with no bit
  change; the `invoke` result is GC-rooted like any `get`/subscript result.
  `reverse` is an in-place word swap. `copy` reuses the existing
  `zap_list_copy`. `index`'s optional bound clamp is emitted in compiled code
  (reusing the slice-bound logic) rather than adding another runtime symbol, so
  the new runtime surface is only `zap_list_pop_*` + `zap_list_reverse`.
- **Positional-only matches Python and the existing surface.** Adding a keyword
  form would diverge from CPython and from `append`/`insert`/`count`/`remove`.

### Rejected alternatives

- **A dedicated `IndexError`-raising `pop` in C that the lowering calls with a
  plain `Call`.** Rejected: the exception would not be catchable (it would
  terminate), violating the done-criteria and the `throwing_call` rule.
- **Returning a raw `i64` word from one untyped `zap_list_pop` and bit-casting in
  the lowering.** Rejected: an `inttoptr` result is a fresh pointer value that
  must be separately GC-rooted; the typed `_ptr` wrapper (an `invoke` dest,
  already rooted) avoids the hazard and matches `zap_list_get_ptr`.
- **A keyword-capable `pop`/`index` with `seam_sigs.rs` rows.** Rejected: these
  are not seam functions, and CPython makes them positional-only.

## Consequences

- The built-in `list` surface now matches CPython's method set except for the
  deliberately-omitted members: **slicing** (`xs[a:b]`) is syntax, not a method
  (already supported, D-0030); **`sort`** already exists (#336); and there is no
  `__reversed__`/`__contains__` method surface (iteration, `in`, `len` are on the
  general paths). Nothing else from CPython's `list` is missing.
- `copy` is shallow by contract: callers needing a deep copy must copy elements
  themselves. This matches CPython's `list.copy()` / `list[:]`.
- Element-kind coverage (`int`/`float`/`bytes`/`str`/reference-struct) and the
  fault paths are pinned by `tests/golden/list_pop_methods.{zap,expected}` and
  the host sema walled-garden suite `crates/sema/tests/list_methods.rs`.

<!-- Related: list sort is #336; the mutation set extend/insert/remove/count/clear is #977 (D-0220, frozen DECISIONS.md). -->
