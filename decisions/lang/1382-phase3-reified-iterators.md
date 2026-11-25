---
title: "Reified per-container iterators — iter() over built-in containers (#1382 phase 3)"
status: Accepted
issue: 1382
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# Reified per-container iterators (#1382 phase 3)

**Status:** Accepted — phase 3 of the `Iterator[T]` epic (RFC Discussion #1417).
Extends the phase-1 protocol (`decisions/lang/1382-iterator-protocol.md`) and the
phase-2 `iter()` / `next()` built-ins (`decisions/lang/1382-iter-next-builtins.md`).
Builds on #1428 (generic-interface conformance at value positions).

## What ships

`iter()` over a **built-in container** now returns a precise **reified iterator**
instead of the phase-2 "phase 3" placeholder error. Every container `for` already
iterates — `list` / `set` / `heap` / `deque` / `oset` / `tuple` / `dict` (keys) /
`str` (char) / `bytes` (int64) / `range` (int64) / array — so `iter(c)` now hands
back an iterator over the same elements, carrying the **exact element type**:

```zap
auto it = iter([1, 2, 3])      # a reified Iterator[int64]
int64 a = next(it)             # exact element type — 1
int64 b = next(it, -1)         # default form
for x in iter(d): ...          # dict: keys
char c = next(iter("ab"))      # str -> char
```

`next(iter(xs))` is statically the exact `T`, which is what #1428's value-position
conformance unblocked. The iterator conforms to `Iterator[T]`, so an `iter()`
result flows into an `Iterator[T]` parameter.

## Representation

Per the phase-1 ADR, the iterator is a **typed synthesized generator** reusing the
free-generator machinery (which carries a precise `(T, bool)` — unlike the
deliberately loose genexpr type). The implementation observation that makes this
cheap: `iter(c)` is semantically exactly the generator expression `(x for x in c)`,
and the genexpr pipeline already lowers one reified iterator per element type with
the **precise** runtime element type (only the genexpr's *sema* type is loose).

- **Lowering** (`crates/ir/src/lower/stmts.rs`, `lower_iter_builtin`): a value that
  is already a protocol iterator keeps phase-2's idempotent `setup_iter` (a live
  view, same object). A container value is bound to a temporary local and fed to
  `lower_genexpr` as `(x for x in <tmp>)`, which synthesizes the generator,
  computes the precise `(T, bool)`, emits its `__next__`, and registers it as its
  own iterator — so `for` / `next` / `sum` consume it through the existing D-0048
  protocol. One path covers every container kind, because `for` already does.
- **sema** (`crates/sema/src/lib.rs`): `iter(container)` is typed as a concrete,
  `$`-prefixed synthetic iterator struct `$iter:<T>` (registered on demand by
  `reified_iter_type`, shared across containers with the same element type) with
  the two `Iterator` dunders over the defaulted element type. The `$` prefix
  mirrors the generator / genexpr synthetic types: it is unspellable and keeps
  `to_ast_type` returning `None`, so monomorphization never forces an
  interface-fat-pointer coercion — yet it conforms structurally to `Iterator[T]`.
  The element type is defaulted (`[1, 2]` → `int64`, not the untyped `int`) so the
  reified iterator conforms to the concrete `Iterator[int64]`.

## Semantics (locked in the phase-1 RFC)

- Single-pass / exhaustible; a live **view** (container reference + cursor), not a
  snapshot — a sequence (`list`/array/`str`/`bytes`/`range`) iterates against the
  **live length** each step (the existing `for` lowering).
- `dict` iterates **keys** by default (matching `for`); `dict`/`set` iteration
  order is unspecified (D-1066), so goldens/diff cases avoid asserting it.
- `dict`/`set` mutation during iteration is undefined in v1 (Go's stance);
  iterators are single-owner / not coroutine-safe (the `--race` detector catches
  misuse).

## Scope

- A non-iterable scalar (`iter(5)`) is a clean `ZAP4024`.
- `array`'s element type is loose (`Unknown`) at sema, as arrays are everywhere;
  it still iterates at runtime through the same path.

## Tests

- Host (sema): `iter_over_a_list_yields_the_precise_element_type`,
  `iter_over_str_bytes_range_yield_precise_elements`,
  `iter_over_a_dict_yields_keys_and_a_set_its_elements`,
  `iter_of_a_container_conforms_to_iterator_interface`,
  `iter_over_a_non_iterable_scalar_is_rejected`; plus the updated
  `iter_next_builtin_codes` pin in `error_codes.rs`.
- Runtime golden (CI LLVM matrix): `tests/golden/iter_reified_containers.zap` —
  list / str / bytes / range / single-entry dict / single-element set, the
  `default` form, and an explicit `next()` raising `StopIteration`.
- CPython differential (deterministic, sequences only):
  `tests/diff/iter_next_containers.zap`.

## Follow-ups

Only phase 4 remains on #1382: exhaustive goldens across every container kind and
the final SPEC/STABILITY pass.
