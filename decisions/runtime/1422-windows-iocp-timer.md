---
title: The fd-less scheduler timer parks on the Windows IOCP poller
status: Accepted
issue: 1422
pr:
date: 2026-11-24
domain: runtime
supersedes:
superseded-by:
---

# The fd-less scheduler timer parks on the Windows IOCP poller

**Status:** Accepted

## Decision

A coroutine timer wait (`zap_time_timer_wait`, behind every coroutine
`time.sleep` / `time.after` / `time.ticker`) now parks on the fd-less scheduler
timer on **Windows (IOCP)** as well as Linux (epoll), freeing the worker M for
the duration of the wait. Previously the park was gated `#if
defined(ZAP_NETPOLL_EPOLL)` and the `#else` returned `ZAP_POLL_CLOSED`, so on
Windows every coroutine timer fell back to a blocking `nanosleep`-class wait that
pinned the OS thread.

The change is a one-line guard widening — `#if defined(ZAP_NETPOLL_EPOLL) ||
defined(ZAP_NETPOLL_IOCP)` — plus comment corrections. No new runtime functions,
no language/seam/sema/lowering changes; `time.sleep` / `after` / `ticker` are
unchanged and simply stop taking the blocking fallback on Windows.

## Rationale

The IOCP poller was already structurally ready; only `zap_time_timer_wait`
opted out. Everything the park needs is backend-agnostic and already driven by
the IOCP poll loop:

- The deadline **min-heap** (`np_heap_push` / `np_compute_timeout` /
  `np_expire_deadlines`, I#1410) is defined outside any backend `#if` — shared.
- The IOCP poll loop (`np_poller_main`, IOCP copy) already calls
  `np_compute_timeout()` to bound its `GetQueuedCompletionStatusEx` wait and
  `np_expire_deadlines()` after each wait — identical to the epoll loop.
- `np_backend_kick` on IOCP is already `zap_iocp_kick()` →
  `PostQueuedCompletionStatus`, so arming a nearer deadline wakes the poller.
- A fd-less timer `PollDesc` (fd/handle = none, never `np_backend_add`-ed) rides
  the shared heap scan regardless of backend registration; the completion-packet
  path never touches it, exactly as on epoll.

So `zap_time_timer_wait`'s body — `np_ensure`, `np_pd_alloc(-1)`, the `g_np_all`
link, `np_heap_push`, `np_backend_kick`, `copark`, `np_all_remove_locked` — is
already all-shared and links on IOCP. Un-gating it is the whole change.

Alternatives rejected:

- **A separate IOCP-specific timer path** (e.g. a waitable timer object / APC):
  needless; the shared heap + poll loop already express the deadline wait, and a
  second mechanism would duplicate the cancel-wake and GC-root paths.
- **Adding a `np_backend_kick()` to `scope_cancel_io` for prompt cancel-wake on
  Windows:** not needed and kept at epoll parity — `scope_cancel_io` wakes the
  waiter's coroutine directly via `coready`; the poller re-reading later is
  harmless. Left unchanged so Windows and Linux behave identically.

## Consequences

- Windows coroutine `time.sleep` / `after` / `ticker` free their worker M and
  consume no handle per timer, matching Linux. The blocking fallback remains only
  for a platform with no deadline-scanning poller (the `#else` stub, e.g. a
  build with neither epoll nor IOCP).
- `time.after` (#1395) and `time.ticker` (#1409) inherit this automatically on
  Windows, since both ride `zap_time_timer_wait`.
- Validation is **Windows-CI-only**: the IOCP path does not compile on
  Linux/macOS, so it is first built on Windows by the merge-queue `windows-runtime`
  (A4) leg, which also runs `zap_win_a4_test.c` — extended here with **T7**, a
  coroutine that sleeps on the scheduler timer and round-trips a sentinel (a
  crash/hang regression guard for the IOCP park). The timer *semantics* are
  proven by the Linux (epoll) `time_sleep_*` / `time_after_*` / `time_ticker_*`
  goldens, which exercise the same shared code path. A richer Windows test that
  asserts the M is actually freed during a coroutine sleep (a MAXPROCS=1
  sibling-progress check, as `time_sleep_detached_frees_m.zap` does on Linux) is a
  follow-up for the Windows harness.

<!-- Related: decisions/runtime/1360-coroutine-sleep-fd-less-timer-park.md (the
     fd-less timer this extends) and decisions/runtime/1410-netpoller-deadline-min-heap.md
     (the shared deadline heap the IOCP loop drives). -->
