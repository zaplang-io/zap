---
title: list.sort / sorted raise catchable MemoryError on a scratch-allocation failure (no abort, no scratch leak)
status: Accepted
issue: 1598
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# `list.sort` / `sorted` scratch-allocation failure is a catchable `MemoryError`

**Status:** Accepted. Applies the #1581 bounded/temp-allocation policy to the
stable-sort runtime. No grammar/surface change; it converts one fail-stop
`abort()` to a catchable raise and closes a missing unwind edge at two call
sites.

## Context

`zap_list_sort_by` (crates/codegen/src/zaprt.c) backs both `list.sort(...)` and
`sorted(...)`. It allocates three temporary index buffers (`idx`, `tmp`, `out`)
with libc `malloc` before any element or key is read or moved. On a `NULL`
result it printed `zap: out of memory sorting a list of N` and called `abort()`,
bypassing the C++ unwinder entirely — a `try`/`except MemoryError` around the
sort could never catch it.

Both lowerings also emitted a plain `Inst::Call` for `zap_list_sort_by`
(crates/ir/src/lower.rs, the `sort` method arm and the `sorted` builtin), so even
after the runtime was taught to raise, the exception would have unwound *past* a
local handler — the same missed-unwind-edge class as #383 / #1566 / #1581.

This is a direct follow-up to #1581 (decisions/lang/1581-memoryerror-reconciliation.md),
which classified a **bounded or temporary** allocation failure as a recoverable
`MemoryError` while the GC's own infrastructure allocation stays fail-stop
(D-0119). The three sort buffers are exactly bounded/temporary scratch: plain
libc `malloc` whose `NULL` is checked before any state is touched, outside the
GC's unrooted-mutator window.

## Decision

A scratch-allocation failure in `zap_list_sort_by` raises a catchable
`MemoryError` (`ZAP_EXC_MEMORY_ERROR`, #882), not `abort()`.

- **Runtime:** the `NULL`-check now `free()`s any of the three buffers that did
  succeed (`free(NULL)` is a no-op) and raises
  `zap_panic_typed(ZAP_EXC_MEMORY_ERROR, zap_str_new("out of memory sorting a list", …))`.
  Because all three allocations precede any read of `keys` or any write to `dst`,
  the input list is left completely untouched on the failure path: after a caught
  `MemoryError` the list still holds its original order and can be sorted again.
  No scratch buffer is leaked on either the success or the failure path.
- **Lowering:** both `list.sort` (the `sort` method arm) and `sorted` (the
  builtin) now lower `zap_list_sort_by` through `throwing_call`, so the call is an
  `invoke` with an unwind edge when a handler (or `main`'s implicit top-level
  handler) is active, and a plain `call` in a frame with none.

## Consequences

- `try`/`except MemoryError` (and `except Exception`, since `MemoryError`
  descends from `Exception`) now recovers a sort scratch-allocation failure, with
  `finally` / `with` cleanup honored on the unwind path. The **uncaught** form is
  unchanged: it prints the normal `Uncaught exception: <message>` diagnostic and
  exits 1 (the uncaught reporter renders only the message string, so the
  `"out of memory sorting a list"` text is preserved).
- A failed in-place `list.sort` leaves the input in its original order — the sort
  had not begun moving elements — so the operation is safely retryable.
- The GC heap-exhaustion `abort()` (D-0119) and every other documented fail-stop
  are unchanged.
- **Behavior change (narrow):** the sort OOM path was a fail-stop `abort()`
  (exit via `SIGABRT`); it is now a catchable `MemoryError`. The path is not
  reachable on a healthy host.
- Tests: host ZIR checks (`crates/ir/src/lib.rs`,
  `lower_sort_alloc_memoryerror_catchable_1598` and
  `sort_plain_call_without_handler_1598`) pin that `list.sort`, bare `list.sort()`,
  and `sorted(...)` lower to `invoke @zap_list_sort_by` with an unwind edge inside
  a `try` (including an `except Exception` clause), and that the same call stays a
  plain `call` in a frame with no active handler. A scratch-allocation failure
  cannot be forced on a healthy host, so there is no end-to-end OOM golden; the
  `sort_catchable_1598` golden locks the success side (a sort inside a
  `try/except MemoryError` runs normally, is re-sortable, and leaves `sorted`'s
  input untouched). Runtime tests run under both Immix and Boehm in CI (the sole
  native gate).

<!-- Related: [lang/1581-memoryerror-reconciliation](1581-memoryerror-reconciliation.md),
[lang/882-memoryerror-exception](882-memoryerror-exception.md),
[lang/1566-catchable-input-errors](1566-catchable-input-errors.md); D-0119
(DECISIONS.md, GC heap-exhaustion fail-stop). -->
