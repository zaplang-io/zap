---
title: A stoppable periodic ticker, time.ticker(d) -> Ticker
status: Accepted
issue: 1409
pr:
date: 2026-11-24
domain: runtime
supersedes:
superseded-by:
---

# A stoppable periodic ticker, `time.ticker(d) -> Ticker`

**Status:** Accepted

## Decision

`time.ticker(d: Duration)` returns a `Ticker` handle that delivers a value on a
channel every `d`, until stopped. It is the periodic companion to the one-shot
`time.after(d)` (#1395); together with the cancellable sleep (#1357/#1360) it
completes the timer API in design/0005.

```python
import time

auto tk = time.ticker(time.seconds(60))
while True:
    select:
        case fired = <-tk.chan:
            flush_metrics()
        case stopping = <-done:
            tk.stop()
            return
```

- **`Ticker` is a prelude `struct`** (reference type) with a receive-channel
  field `chan` (`channel[int64]`, delivering the monotonic-ns instant of each
  tick — same element as `time.after`), an internal `_done` channel, and a
  `stop()` method. A reference struct, so copies share the same channels.
- **Fixed schedule:** ticks target absolute multiples of `d` (`next = start +
  n*d`), so per-tick wake latency does not accumulate as drift. When the consumer
  has fallen more than one interval behind, the loop resyncs to the next future
  deadline instead of firing a catch-up burst.
- **Drop on slow consumer:** `chan` is cap-1 and the firing send is non-blocking
  (a `select` with a `default`); if the previous tick is undrained the new one is
  dropped (Go `Ticker` semantics). The ticker stays on schedule and memory is
  bounded.
- **`stop()`** ends the firing loop. It is idempotent and race-free: it sends on
  the cap-1 `_done` via a `select`/`default`, so a repeat (or a stop after the
  loop already drained the signal) simply drops. There is no `close()` and thus
  no double-stop fault.
- **Resolution** is millisecond-class and **fd-less** (it reuses `time.after`,
  hence #1360's fd-less scheduler timer), so many concurrent tickers consume no
  file descriptors. `d <= 0` yields a ticker that never fires (the loop returns
  immediately) rather than spinning.

## Rationale

The firing loop must wait for *either* the next deadline *or* a stop, and must
stop promptly even while parked between ticks. A `select` over a timer **channel**
and a `done` channel expresses exactly that, and Zap already has every piece:
`time.after` gives the timer as a channel, `select` multiplexes it against
`done`, closing/sending wakes a parked `select` at once, and `channel`/`spawn`
build the rest. So the whole `Ticker` — struct, `stop()`, and the firing loop —
is written in **Zap prelude source** (`PRELUDE_SRC`), the `Thread` precedent
(a prelude struct whose method `spawn detached`s a prelude loop function). The
only Rust is the `time.ticker` seam, which assembles the two channels, spawns the
prelude loop `__ticker_fire` **detached**, and wraps them in the `Ticker` struct —
all via existing lowering primitives (`zap_chan_new`, `Inst::Spawn`, `StructNew`/
`FieldSet`); no new runtime C.

A C firing loop was rejected: the runtime has no way to wait on a timer *and* a
channel together (the fd-less timer is a `PollDesc`, not a channel, and `zap_select`
cannot take a timer), so a prompt stop would need new runtime machinery. The Zap
`select`-over-`after`+`done` loop gets prompt stop for free.

The firing coroutine is **detached** (like `after`'s), not scope-owned: a detached
coroutine is never joined at program exit, so a never-stopped ticker is terminated
at exit rather than hanging the exit join a scope-owned infinite loop would cause.

Alternatives rejected:

- **A bare leaky `time.tick(d) -> channel`** (Go `time.Tick`) with no stop: leaks
  the firing coroutine for the program's life; unacceptable as the only form.
- **Blocking (backpressure) on a slow consumer:** a slow consumer would stretch
  the cadence, defeating a *periodic* timer. Drop keeps the schedule.
- **`stop()` via `close(_done)`:** a second `close` (or a stop after the scope
  already ended it) faults; the cap-1 `select`/`default` send is idempotent.
- **Carrying a wall-clock `datetime`:** the monotonic-ns `int64` is cheaper and
  matches `time.after`; the tick value is usually discarded.
- **A C runtime ticker:** see above — no clean timer-or-channel wait in C.

## Consequences

- New prelude source in `crates/syntax/src/parser.rs` `PRELUDE_SRC`: `struct
  Ticker` (+ `stop()`) and `def __ticker_fire(...)`; new needles `"ticker"` /
  `"Ticker"` so the surface is injected when referenced.
- New seam wiring: sema (`crates/sema/src/modules/time.rs`, returns
  `Ty::Named("Ticker")`), lowering (`crates/ir/src/lower/modules/time.rs`:
  two `zap_chan_new`, a detached `Inst::Spawn` of `__ticker_fire`, `StructNew`/
  `FieldSet` of the handle), and the seam row `("time", "ticker", &["d"])`.
- No new runtime C and no `select`/channel changes. The ticker reuses
  `time.after` per tick, so it inherits the one-shot timer's fd-less park; a
  per-tick `after` allocation is acceptable at the coarse intervals tickers use
  and is subsumed by the timer-heap follow-up (#1410).
- Linux (epoll) only in v1 via the shared timer; non-Linux keeps the blocking
  fallback inside each `after`. A deadline-keyed timer heap (#1410) and a
  Windows/IOCP backend remain open.

<!-- Related: decisions/runtime/1395-one-shot-timer-channel-after.md (the one-shot
     timer this builds on) and
     decisions/runtime/1360-coroutine-sleep-fd-less-timer-park.md (the fd-less
     scheduler timer underneath). -->
