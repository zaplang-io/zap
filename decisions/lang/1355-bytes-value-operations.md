---
title: sum, sorted, and `in` membership over a bytes value
status: Locked
issue: 1355
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# `sum`, `sorted`, and `in` membership over a `bytes` value

**Status:** Locked

## Decision

Three operations that take a `bytes` value **directly** — rather than through
the D-0048 iteration protocol that #1349 wired up — now lower instead of failing
code generation, matching Python:

- **`sum(bytes[, start])`** reduces the byte values to an `int64` (each byte
  `0..=255`), seeded by the optional numeric `start` (else `0`). Empty `bytes`
  sums to `start`/`0`.
- **`sorted(bytes)`** returns a fresh **`list[int64]`** of the byte values in
  ascending order; `reverse=` and `key=` apply as for a `list`. Empty `bytes`
  gives `[]`.
- **`x in bytes` / `x not in bytes`** is a `bool`: an **`int`** left operand is
  **byte-value membership** (is some byte equal to `x`, `x` taken as a byte
  value), and a **`bytes`** left operand is **subsequence membership** (is the
  left a contiguous byte run of the right; the empty `bytes` is always present).
  `not in` negates.

Implementation reuses the #1349 machinery: `sum` and the `sorted`
materialization iterate through `iterate_bytes` (unsigned `zap_bytes_index`),
and `bytes in bytes` reuses `zap_str_contains` (a byte-level `memmem`, since
`bytes` and `str` share the `ZapStr` buffer). No new runtime entry point is
added. Sema types `sum(bytes) → int64`, `sorted(bytes) → list[int64]`, and
membership `→ bool`.

## Rationale

#1349 made `bytes` iterable, so `for b in data`, comprehensions, generator
expressions, `*bytes` splats, and the `set`/`oset` constructors all work. But
`sum`, `sorted`, and `in` consume a value through their own lowering, not the
shared `iterate` protocol, and each rejected a `bytes` operand in codegen even
though sema accepted it (sema's `sum`/`sorted`/`in` typing never gated on the
operand). That left the same accept-then-fail class #1349 fixed, now glaring:
`sum(c for c in b"abc")` compiled while `sum(b"abc")` did not. These are the
natural Python operations on an iterable of small integers, so they are
implemented rather than closed off.

Design points weighed:

- **`sum` element type is `int64`, not `bytes`.** `sum` reduces the *elements*;
  a `bytes` iterates as `int64`s (#1349), so the accumulator and result are
  `int64` — the same answer `sum([b for b in data])` gives.
- **`sorted(bytes)` yields `list[int64]`, not `bytes`.** Python's
  `sorted(b"...")` returns a `list` of ints, not a `bytes`; a sorted-bytes
  *value* would be `bytes(sorted(data))`. Mirroring Python keeps `sorted`
  uniformly "iterable → sorted list of its elements". The bytes are materialized
  into a **fresh** `list[int64]` (already private, so no `zap_list_copy`) and
  sorted in place through the existing `prepare_sort` / `zap_list_sort_by` path,
  so `reverse=`/`key=` come for free.
- **`int in bytes` is byte-value membership, `bytes in bytes` is subsequence.**
  This matches Python exactly: `0x55 in b"UTC"` is `True`, `b"TC" in b"UTC"` is
  `True`, `b"" in b"UTC"` is `True`. The int scan compares each unsigned byte
  (so `255 in bytes([0, 255])` is `True`, never a sign-extension miss); the
  bytes search is the raw-byte `memmem` already backing `sub in str`. A
  non-`int`/non-`bytes` left operand (e.g. `str`, `float`) still errors in
  codegen — the same accept-then-fail the permissive `In => bool` sema rule
  leaves for every nonsensical membership, not a regression introduced here, and
  tightening that rule is out of scope.

## Consequences

- `sum(data)`, `sorted(data)` (and `sorted(data, reverse=True)`), and `b in data`
  / `b not in data` over a `bytes` `data` all compile and run; `min(bytes)` /
  `max(bytes)` remain the documented ZAP4024 follow-up (the iterable form of
  min/max over any non-list is a separate gap).
- No new runtime helper; `iterate_bytes`, `zap_bytes_index`, `zap_str_len`, and
  `zap_str_contains` already exist.
- The bytes iteration surface (#1349) and these value operations together make
  `bytes` behave as a Python `bytes` wherever it is consumed as a sequence of
  small integers.

<!-- Related: decisions/lang/1349-bytes-iteration (bytes is iterable), I#337 (bytes indexing / zap_bytes_index), #1344 (bool sign-extension), D-0048 (the Iterable protocol). -->
