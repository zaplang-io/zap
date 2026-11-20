---
title: net TCP keepalive interval tuning — idle / interval / count
status: Locked
issue: 1141
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# `net` TCP keepalive interval tuning

**Status:** Locked

## Decision

Add keepalive-timing setters/getters to `net`, the follow-up #1141 to #994 (which
shipped `set_keepalive` as an on/off toggle only). In the same named-option style
as #994:

- `net.set_keepalive_idle(sock, seconds)` — idle time before the first probe.
- `net.set_keepalive_interval(sock, seconds)` — interval between probes.
- `net.set_keepalive_count(sock, count)` — failed probes before the connection
  is dropped.
- `net.get_keepalive_idle(sock)` / `get_keepalive_interval(sock)` /
  `get_keepalive_count(sock)` → `int64` (seconds / count).

Values are seconds (idle, interval) or a count; a non-positive value raises
`ValueError`. Call `net.set_keepalive(sock, True)` first for them to take effect.
Reuses the #994 `setsockopt`/`getsockopt` helpers.

## Rationale

- **The missing half of #994's keepalive.** `SO_KEEPALIVE` alone leaves probe
  timing at the OS default (a ~2h idle on Linux), useless for detecting a dead
  peer in a useful window. The three knobs are what make keepalive actionable.
- **POSIX subset, not the Windows ioctl.** idle is `TCP_KEEPIDLE` on Linux and
  `TCP_KEEPALIVE` on macOS/BSD (both seconds, selected by a compile-time macro);
  interval/count are `TCP_KEEPINTVL`/`TCP_KEEPCNT`. A knob the platform does not
  expose raises a catchable "not supported" — on Windows all three do, since its
  `SIO_KEEPALIVE_VALS` ioctl takes milliseconds and has no count. Implementing
  that ioctl path is a follow-up; the POSIX knobs cover the real targets and read
  back the exact value set (unlike the OS-rounded buffer sizes).
- **Positive-only, validated before the syscall.** A zero or negative idle /
  interval / count is meaningless (and `TCP_KEEPCNT = 0` is rejected by the
  kernel anyway), so it is a `ValueError` up front — clearer than a late
  setsockopt `EINVAL`.

## Consequences

- Long-lived idle connections can be given a useful dead-peer detection window.
- A Windows `SIO_KEEPALIVE_VALS` implementation (idle + interval; no count) is a
  follow-up.
- `net` remains **Experimental** (STABILITY.md §4.4). Follow-up to
  [stdlib/994-socket-options.md](994-socket-options.md).
