---
title: A deadline min-heap for the netpoller timer registry
status: Accepted
issue: 1410
pr:
date: 2026-11-24
domain: runtime
supersedes:
superseded-by:
---

# A deadline min-heap for the netpoller timer registry

**Status:** Accepted

## Context

Every coroutine sleep (#1360) and one-shot timer (#1395), and every fd I/O wait
with a deadline, arms a `PollDesc` deadline. The poller found the nearest deadline
(`np_compute_timeout`, for `epoll_wait`'s timeout) and woke expired waiters
(`np_expire_deadlines`) by scanning the whole live-descriptor list `g_np_all` —
**O(n)** per poll tick in the number of live descriptors. Fine for a handful of
timers; wasteful when thousands of coroutines sleep or arm timers concurrently.

## Decision

Add a binary min-heap keyed on absolute `CLOCK_MONOTONIC` deadline. Arming a
deadline pushes one entry `{deadline, pd, is_read}`; the nearest deadline is the
heap root (**O(1)**), and expiry pops due roots (**O(log n)** each). This replaces
both linear scans.

The heap is **additive**, not a replacement for `g_np_all`. `g_np_all` still backs
scope-cancellation wakeups (`scope_cancel_io`) and GC root marking
(`zap_netpoll_mark_roots`), both of which must enumerate every live descriptor; the
heap only optimizes the deadline scan.

### Lazy deletion

A readiness wake or a scope cancel clears a waiter's slot but does **not** remove
its heap entry. Stale entries are reclaimed when the poller pops them and
validates, under `pd->mu`, that the slot is still parked with that exact deadline;
a mismatch discards the entry. This is safe because:

- PollDescs are pooled and never `free`d, so a heap entry's `pd` stays
  dereferenceable even after the descriptor is recycled.
- A recycled pd is re-armed with a fresh (later) deadline, so a stale entry fails
  the deadline-equality check.
- An entry whose deadline is already due can only match a slot whose own deadline
  has likewise passed, so waking it is correct even across a recycle.

`np_pd_alloc` resets the recycled descriptor's fields under `pd->mu` so a
concurrent poller validation of a lingering stale entry never races the reset
(the former g_np_mu-guarded linear scan avoided this implicitly).

### Locking

Lock order is `pd->mu` → `g_np_heap_mu` on the arm path (push while the slot is
armed). The poller pops under `g_np_heap_mu`, **releases it**, then takes `pd->mu`
to validate and wake, so the two are never held together on the poller side and no
cycle forms with the existing `g_np_mu` → `pd->mu` registry order. Heap entries
hold only an infra pointer and integers (no GC pointers), so the heap needs no GC
tracing.

## Consequences

- No surface change; behavior and all existing goldens are unchanged. The heap
  root may transiently be a stale entry, which only yields a slightly earlier
  `epoll_wait` wake (harmless — expiry then prunes it).
- **Known trade-off (lazy deletion):** an entry for a waiter that finished *before*
  its deadline (fd I/O completing early, or an early-cancelled long sleep) lingers
  until that deadline elapses, so the heap can hold more entries than there are
  live waiters. For the primary target — many concurrent sleepers — this is
  self-limiting, since a sleep's entry is popped exactly when it fires. A future
  refinement can make removal active (store each slot's heap index and remove on
  clear) to bound the heap at the live-waiter count; it is deferred because the
  multi-site index maintenance is materially more error-prone and this path is
  validated only on CI (no local LLVM).
- Scope: all in `crates/codegen/src/zaprt.c` (netpoller). No language surface, no
  seam, no sema/lowering changes.
