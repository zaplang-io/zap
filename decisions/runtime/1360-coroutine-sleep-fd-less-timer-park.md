---
title: Every coroutine sleep parks on an fd-less scheduler timer
status: Accepted
issue: 1360
pr:
date: 2026-11-24
domain: runtime
supersedes:
superseded-by:
---

# Every coroutine sleep parks on an fd-less scheduler timer

**Status:** Accepted

## Decision

A `time.sleep` / `sleep_ns` / `sleep_for` running on **any coroutine** (whether
or not it has an owning `scope:`) now parks the coroutine's G on a fd-less
scheduler timer until its absolute `CLOCK_MONOTONIC` deadline, freeing the worker
M so sibling coroutines run during the wait. This generalizes
decisions/runtime/1357-cancellable-coroutine-sleep.md, which freed the M only for
sleeps inside an owning scope; a scope-less coroutine sleep (e.g. a
`spawn detached` coroutine) previously fell back to the blocking `nanosleep` and
pinned its M.

The timer is a `PollDesc` with **no fd**: it is linked into the netpoller's live
registry (`g_np_all`) only so the existing deadline scan (`np_compute_timeout` /
`np_expire_deadlines`) wakes it at the deadline, `scope_cancel_io` wakes it on a
scope cancellation (raising `Cancelled` when the coroutine has a scope), and
`zap_netpoll_mark_roots` pins its borrowed scope under Immix — but it is never
handed to `np_backend_add`, so it consumes no epoll/IOCP registration and no file
descriptor. The internal entry point is `zap_time_timer_wait(deadline_ns)`.

The blocking `nanosleep` remains only for a **native thread** (`main`, not on an
M) and for platforms without the epoll poller (non-Linux v1), where
`zap_time_timer_wait` returns `ZAP_POLL_CLOSED` and the caller blocks. Resolution
on the parked path stays millisecond-class; the native-thread path stays
nanosecond-exact. `ns <= 0` on a coroutine is a cooperative checkpoint. The
`time.sleep*` surface is unchanged.

## Rationale

#1357 proved the cooperative, cancellable park on the netpoller, but backed each
sleep with a real pipe fd and gated it on the sleep having an owning scope. Two
limits motivated this change:

- **Fd cost.** An fd-per-sleep park cannot be the default: thousands of
  concurrent sleepers would exhaust the process's file descriptors (`EMFILE`).
  An fd-less registry entry removes that ceiling.
- **Scope-less coroutines still pinned their M.** A detached coroutine (or any
  coroutine without a scope) that slept blocked its worker, defeating the M:N
  scheduler for a common pattern. Freeing the M is independent of whether there
  is anything to cancel, so it should not require a scope.

Alternatives rejected:

- **A dedicated timer mutex/heap registry separate from the netpoller.** More
  code and a second park/wake path to keep correct; the fd-less `PollDesc` reuses
  the netpoller's already-validated deadline scan, cancellation wake, and GC-root
  marking verbatim.
- **A timer heap now.** The linear registry scan the netpoller already runs each
  poll iteration is reused unchanged; it is O(n) over live descriptors, adequate
  for the current workloads. A min-heap keyed on deadline, for very large numbers
  of simultaneous sleepers, is a documented optimization follow-up, not required
  here.
- **Keep the #1357 pipe park for in-scope sleeps and add the fd-less path only
  for scope-less ones.** Two mechanisms for one feature; the fd-less park
  subsumes both, so #1357's pipe park is removed.

## Consequences

- `zap_time_sleep_cancellable_ns` no longer opens a pipe; it calls
  `zap_time_timer_wait`. The #1357 pipe/`fcntl` park is gone.
- Behavior for in-scope sleeps is unchanged (same cancellation, same M-freeing,
  same ms-class resolution); the #1357 goldens continue to hold.
- Scope-less / detached coroutine sleeps now free their M (new behavior, covered
  by `tests/golden/time_sleep_detached_frees_m.zap`); many concurrent sleepers
  consume no fds (`tests/golden/time_sleep_many.zap`).
- Linux (epoll) only in v1; non-Linux keeps the blocking fallback. The IOCP
  poller does not run the deadline scan, so `zap_time_timer_wait` returns
  `CLOSED` there — wiring Windows timers is a follow-up.
- A timer heap for very many concurrent sleepers remains open.

<!-- Related: decisions/runtime/1357-cancellable-coroutine-sleep.md (the in-scope
     cancellable park this generalizes). -->
