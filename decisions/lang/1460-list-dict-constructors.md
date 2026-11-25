---
title: list(iterable) and dict(pairs / copy / **kwargs) constructors
status: Locked
issue: 1460
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `list(iterable)` / `dict(...)`

**Status:** Locked. Batch C of the overload-parity epic #1386.

## Decision

Add the value-constructor call forms for `list` and `dict`, mirroring the
existing `set(iterable)` (`set` was already implemented). The subscripted
type-constructors `list[T]()` / `dict[K,V]()` (empty collections) are unchanged;
these are the bare `list(x)` / `dict(x)` call forms.

### `list(iterable)`
A fresh `list[T]` of the iterable's elements **in order**, `T` inferred from the
element type — the list-shaped twin of `set(iterable)`. Empty is `list[T]()`
(sema points there, as it does for `set()`).

### `dict(...)` — three forms
- **`dict(pairs)`**: an iterable of 2-tuples `(K, V)` → `dict[K, V]`. Each
  element must be a 2-tuple; a non-pair element is a `ZAP4024` type error.
- **`dict(other_dict)`**: a copy with the same `K, V` (reuses the `**`-merge
  path, `merge_dict_into`).
- **`dict(key=value, ...)`**: the keyword arguments' names are the string keys →
  `dict[str, V]`, all values sharing `V`.
- Positional and keyword forms cannot be mixed (`ZAP4025`). Empty is
  `dict[K,V]()`.

## Rationale

- **Mirror `set`, reuse the builders.** `list(iterable)` reuses the list
  literal's element-kind/boxing (`zap_list_push_*`); `dict` reuses the dict
  literal's `zap_dict_set_*` + key-fn machinery and `merge_dict_into`. No new
  runtime entry points.
- **`dict(**kwargs)` without new kwarg machinery.** The keyword call-args are
  read directly in the arm (names → `str` key constants), so no general
  `**kwargs`-collection feature is needed; it is specific to `dict(...)`.
- **Iteration order honoured where it exists.** `list(...)` preserves order for
  ordered sources; for an unordered source (a `set`, a `dict`'s keys) the
  element order is unspecified (#1066), exactly as iterating that source is —
  `list(...)` adds no ordering guarantee a `for` loop would not give.

## Implementation notes

- sema: new `list` and `dict` value-constructor arms next to `set` in the
  `infer_call` name match.
- Lowering: `lower_list_from_iterable` and `lower_dict_from_call`. The latter
  distinguishes a dict-copy from a pairs iterable by the operand's type, so the
  shared value-level iterator was factored out of `iterate` into
  `iterate_value(val, ty, body)` (a behaviour-preserving extraction — `iterate`
  now evaluates the expression and delegates). `dict(pairs)` decomposes each
  2-tuple with `field_get` over `ctx.tuples.fields(sid)`.

## Consequences

- No new runtime kernels (the existing `zap_list_*` / `zap_dict_*` entry points
  cover it); still LLVM-CI-gated for the end-to-end goldens.
- Covered by golden `list_dict_ctors`, CPython differential `list_dict_ctors`
  (deterministic cases), and sema rejection tests `list_dict_ctor_codes`.
- `tuple(iterable)` (#1461) stays **deferred** — a dynamic iterable has no
  fixed-arity tuple type; it needs a new immutable-sequence type (a separate
  maintainer decision). This completes the mechanical batch of epic #1386
  (#1458, #1459, #1460); `sum`/`complex`/`set` were already done.
