---
title: Coroutine time.sleep parks on a cancellable scheduler timer
status: Accepted
issue: 1357
pr:
date: 2026-11-24
domain: runtime
supersedes:
superseded-by:
---

# Coroutine time.sleep parks on a cancellable scheduler timer

**Status:** Accepted

## Decision

`time.sleep`, `time.sleep_ns`, and `time.sleep_for`, when called on a coroutine
that has an **owning `scope:`**, park the coroutine's G on the netpoller timer
until an absolute `CLOCK_MONOTONIC` deadline. This (a) frees the worker **M** so
sibling coroutines on that worker run during the wait, and (b) makes the sleep a
**cancellation point**: a `scope(timeout=)` deadline, a `cancel_on` signal, or a
sibling failure wakes the wait at once and raises `Cancelled` (surfaced to the
caller as `ScopeTimeout` under a timed scope, per the #825 sever rules). On a
native thread (`main`), on a coroutine with no owning scope, or on a platform
without the netpoller (non-Linux v1), the sleeps keep the existing blocking
`nanosleep`. A non-positive or already-past duration is a cooperative checkpoint
(observe a pending cancel, then yield once), not a silent no-op.

Resolution of the parked (cancellable) path is **millisecond-class** — the wake
rides the poller's `epoll_wait` tick. The blocking `nanosleep` fallback stays
nanosecond-exact. The `time.sleep*` surface is unchanged (no new function, same
positional/keyword arguments), so `crates/syntax/src/seam_sigs.rs` is untouched.

## Rationale

The pre-existing sleep lowered to a blocking `nanosleep` that pinned the OS
thread and never observed cancellation, so a coroutine sleeping inside
`scope(timeout=, grace=)` could neither drain cooperatively on cancellation nor
yield its M to siblings — the two defects #1357 targets (consumer: the otel #3
periodic metrics reader).

The runtime already had the exact primitive needed: the netpoller
(`zap_netpoll_wait`) parks a G on an absolute monotonic deadline, frees the M, is
GC-safe, and its `scope_cancel_io` path already wakes scope-cancelled waiters.
Reusing it makes cancellation fall out of the same park that frees the M — they
are one mechanism, not two.

Alternatives rejected:

- **A new name (`time.sleep_cancellable`), leaving `sleep*` blocking.** Leaves
  the obvious call (`sleep_for`) pinning the M as the default — the footgun stays
  the default and only the caller who knows to reach for the variant escapes it.
  Making the plain sleep correct is the point.
- **Return early on cancel instead of raising.** A cancelled sleep would be
  indistinguishable from a normal wake, so a periodic loop would run one extra
  iteration on its way out. Raising matches every other cancellation point
  (channels, `select`, I/O).
- **An fd-less timer registry for this issue.** Lower fd cost, but it is net-new
  poller-thread code; for #1357's bounded set of cancellable sleepers an
  fd-backed park (a fresh never-ready pipe per wait, mirroring the netpoll
  self-test's deadline probe) reuses only validated paths. The fd-less timer is
  the right default once *every* coroutine sleep frees its M — see #1360.

## Consequences

- `time.sleep*` can now raise `Cancelled` / `ScopeTimeout`, but only under
  cancellation. Code that sleeps inside a cancellable scope should treat the
  sleep as a cancellation point (use `finally` / `with` for cleanup).
- The cancellable path is ms-class; callers needing ns-exact short sleeps get
  that only on the native-thread path (no owning scope).
- A coroutine sleep with **no** owning scope still pins its M (blocking path).
  Generalizing M-freeing to every coroutine sleep, plus an fd-less timer heap to
  scale it, is tracked in **#1360**, sequenced after this lands and built on this
  park.
- Any future `cancel_on` trigger (#492, channel/token) must deliver its cancel
  through `scope_request_cancel` (which calls `scope_cancel_io`) to wake a
  timer-parked sleep; `scope(timeout=)` stays the canonical deadline spelling so
  it does not grow redundant with a timer-channel `cancel_on`.
- A general `time.after(d) -> chan` one-shot timer channel (for `select`) remains
  open (the `after()` half of the design's "General timer API"); not part of
  #1357.

<!-- Related: [runtime/1100-channel-element-type](./1100-channel-element-type.md); scope sever semantics live with #825. -->
