---
title: A one-shot timer channel, time.after(d) -> channel[int64]
status: Accepted
issue: 1395
pr:
date: 2026-11-24
domain: runtime
supersedes:
superseded-by:
---

# A one-shot timer channel, `time.after(d) -> channel[int64]`

**Status:** Accepted

## Decision

`time.after(d: Duration)` returns a buffered (capacity 1) `channel[int64]` that
receives **exactly one** value — the `CLOCK_MONOTONIC` instant (ns) at which the
timer fired — after the duration `d`, then blocks on any further receive (the
channel is never closed). This is the Go `time.After` shape and the `after()`
half of the General timer API (design/0005; the `sleep()` half is #1357/#1360).
It exists so a coroutine can compose a timeout with `select`:

```python
import time

select:
    case v = <-work:
        handle(v)
    case fired = <-time.after(time.seconds(5)):
        raise TimeoutError()
```

`select` needs no special support: a timer channel is an ordinary channel that
happens to receive one send, so the firing wakes a parked `select` through the
normal receiver path.

**Spelling.** `time.after`, in the `time` module, consistent with `time.sleep`/
`sleep_for` — not a bare `after` built-in. (design/0005 had listed a bare
`after`; that was updated to `time.after`, which avoids a new global that could
shadow the common identifier `after`.)

**Fire value.** The channel carries the monotonic-ns fire instant as an `int64`.
This is the cheapest element (reuses the `i64` word channel entry points, no GC
element, no per-fire allocation) and still lets a receiver measure elapsed time.
Go returns a wall-clock `Time`; the monotonic ns is the more correct quantity
for a timer and the `select` idiom usually discards it anyway.

**Buffering / lifetime.** Capacity 1, so the firing send never blocks even if
nobody receives — a timer nobody waits on neither leaks nor stalls. The firing
coroutine is `spawn detached` (no owning scope), so it is **not** part of the
program-exit join: a pending long timer is terminated when `main` returns and
never delays exit.

**Resolution / fallback.** The wait reuses the fd-less scheduler timer
(`zap_time_timer_wait`, #1360), so resolution is millisecond-class and no file
descriptor is consumed per timer. On a non-epoll platform that call returns
`CLOSED` and the firing coroutine falls back to a blocking sleep on its own M,
exactly as `time.sleep` does there. `d <= 0` fires effectively immediately (the
absolute deadline is already in the past).

## Rationale

The timer machinery to make this cancellable, scheduler-aware, and fd-less
already exists from #1357/#1360; `after` is the channel-shaped surface over it.
A single runtime entry point `zap_time_after(ns)` creates the channel and a
detached firing coroutine, keeping the lowering trivial (a plain `Call`
returning `Ty::Chan(Elem::I64)` — it does not park in the caller, so it is not a
`throwing_call`) and adding nothing to `select`.

Alternatives rejected:

- **Bare `after` built-in** (as design/0005 first sketched). A new global that
  shadows the ordinary identifier `after`; every other timer operation is under
  `time.`, so `time.after` is the consistent, collision-free home.
- **Carrying a wall-clock `datetime`** (closest Go parity). Needs a GC-pointer
  channel element and a datetime allocation per fire for a value the timeout
  idiom discards; the monotonic-ns `int64` is cheaper and more correct.
- **The timer fires by the poller performing the channel send directly** (no
  coroutine). The send would run under poller locks against the channel mutex —
  a deadlock hazard and a second send path to keep correct. A detached firing
  coroutine does the send in ordinary coroutine context and reuses the existing,
  already-validated send path.
- **An unbuffered or closed-on-fire channel.** Unbuffered would let a timer
  nobody receives block its firing coroutine forever (a leak); closing on fire
  would make a second receive observe a ready, zero-value channel rather than
  block. Cap-1-and-never-closed matches Go and has neither problem.

## Consequences

- New runtime: `zap_time_after(int64 ns) -> channel`, plus the detached firing
  trampoline and a 2-word GC descriptor for its argument (the channel pointer is
  traced), in `crates/codegen/src/runtime/time_datetime.inc.c`. The
  channel-then-wrapper allocation order follows `zap_signal_notify`.
- New surface wired in lockstep: sema (`crates/sema/src/modules/time.rs`,
  returns `Ty::Chan(Box::new(Ty::Int64))`), lowering
  (`crates/ir/src/lower/modules/time.rs`, emits `zap_time_after`), codegen
  `declare_rt` (`crates/codegen/src/modules/time.rs`), and the seam row
  `("time", "after", &["d"])` (`crates/syntax/src/seam_sigs.rs`).
- `select` and the channel runtime are unchanged.
- Linux (epoll) only in v1; non-Linux keeps the blocking fallback inside the
  firing coroutine (its own M). A periodic ticker, a deadline-keyed timer heap
  for very many concurrent timers, and a Windows/IOCP timer backend remain open.

<!-- Related: decisions/runtime/1360-coroutine-sleep-fd-less-timer-park.md (the
     fd-less scheduler timer this reuses) and
     decisions/runtime/1100-channel-element-type.md (the channel element typing
     the i64 fire value rides on). -->
