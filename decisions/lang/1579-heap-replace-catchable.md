---
title: Empty heap.replace raises a catchable IndexError instead of aborting
status: Accepted
issue: 1579
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Empty `heap.replace` is catchable

**Status:** Accepted. Resolves the first of the three policy items **Held** under
[lang/1566-catchable-input-errors](1566-catchable-input-errors.md).

## Decision

Calling `heap.replace(x)` on an empty `heap[T]` raises a **catchable
`IndexError`** — exactly like `heap.pop()` / `heap.peek()` on an empty heap —
instead of terminating the process. A matching `try`/`except` in the same
function handles it, with `finally` and `with`/context cleanup running on the
unwind path, and execution continues past the handler.

This **supersedes the D-0236 contract** (which documented empty `replace` as
aborting like the then-aborting `pop`). D-0236 lives in the frozen `DECISIONS.md`
and is not edited; this ADR is the current authority for the empty-`replace`
behavior. Maintainer-approved (#1579).

`heap.pushpop(x)` is **unchanged**: it is total on the empty heap (it returns `x`
and leaves the heap intact), never raises, and keeps its plain lowering.

## Rationale

- **Congruence.** `replace` was the only remaining heap misuse that aborted
  after #1566 made `pop`/`peek` catchable. Empty-`replace` is an ordinary
  recoverable state error — the same class as indexing an empty sequence — so
  `IndexError` is the least-surprising type and matches the sibling operations.
- **Two defects, same shape as #1566.** (1) The runtime entry points
  `zap_heap_replace_{i64,ptr,f64}` guarded with the aborting
  `zap_heap_require(h, "replace")` now use the raising
  `zap_heap_require_live(h, "replace")` (`zap_panic_typed(ZAP_EXC_INDEX_ERROR,
  …)`), the guard already shared by `pop`/`peek`; the now-unused aborting
  `zap_heap_require` was removed. (2) The `lower.rs` heap-method dispatch emitted
  a plain `Inst::Call` for both `replace` and `pushpop`; `replace` now lowers
  through `throwing_call` (an `Inst::Invoke` whose unwind edge targets the active
  landing pad inside a `try`), while `pushpop` stays a plain `call`. A missed
  unwind edge would terminate instead of catching — the exact #1566 failure mode
  — so the split is pinned by host ZIR checks.

## Consequences

- `except IndexError`-based recovery now works for empty `replace`; the
  **uncaught** form still prints the normal `Uncaught exception: <message>`
  diagnostic ("heap replace on an empty heap") and exits 1.
- Non-empty `replace` is semantically unchanged, as is `pushpop` on any heap.
- Runtime behavior is pinned under both Immix and Boehm by
  `tests/golden/heap_replace_catchable.{zap,expected}` (caught + `finally` +
  continuation across i64/str/f64 heaps, plus the unaffected `pushpop`) and the
  uncaught `heap_replace_uncaught.{zap,expected,exit,stderr}` (exit 1); the
  invoke/plain-call split is pinned by
  `crates/ir/src/lib.rs::lower_heap_replace_is_catchable_pushpop_is_not`.

<!-- Related: [lang/1566-catchable-input-errors](1566-catchable-input-errors.md). -->
