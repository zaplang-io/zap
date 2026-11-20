---
title: net keepalive timing on Windows — SIO_KEEPALIVE_VALS (idle + interval)
status: Locked
issue: 1157
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# `net` keepalive timing on Windows

**Status:** Locked

## Decision

Implement the Windows branch of the #1141 keepalive-timing surface:

- `net.set_keepalive_idle(sock, seconds)` and `net.set_keepalive_interval(sock,
  seconds)` route to the `SIO_KEEPALIVE_VALS` `WSAIoctl` (via `<mstcpip.h>`) on
  Windows, matching CPython's `socket.ioctl(SIO_KEEPALIVE_VALS, …)`. The ioctl
  sets the idle time and the inter-probe interval **together** (in milliseconds)
  and also turns keepalive on, so each setter stashes its own value on the handle
  (`ZapSock.ka_idle_ms` / `ka_intvl_ms`, 0 = Windows default) and re-applies both.
- `net.set_keepalive_count` and the three `get_keepalive_*` readers raise a
  catchable "not supported" error on Windows: Windows has no probe-count knob and
  no symmetric read for the ioctl values.

POSIX is unchanged (per-knob `TCP_KEEPIDLE`/`TCP_KEEPINTVL`/`TCP_KEEPCNT`
setsockopt options, #1141).

## Rationale

- **Matches CPython's Windows reality.** CPython exposes none of
  `TCP_KEEPIDLE`/`TCP_KEEPINTVL`/`TCP_KEEPCNT` on Windows; its only keepalive-
  timing API there is `sock.ioctl(SIO_KEEPALIVE_VALS, (onoff, time_ms,
  interval_ms))` — idle+interval together, milliseconds, no count. Zap uses the
  same ioctl but keeps the *same named setters* as POSIX, so callers need no
  Windows-only method (a small ergonomic win over CPython).
- **`SIO_KEEPALIVE_VALS`, not `setsockopt(TCP_KEEPIDLE)`.** Modern Windows
  (10 1709+) does accept the `TCP_KEEP*` setsockopt options, but the MinGW-w64
  toolchain the Windows CI uses does not reliably define those constants, and the
  ioctl works on every supported Windows. The ioctl is the portable choice.
- **Count / getters raise.** Honest about what the platform lacks, consistent
  with the "unsupported knob raises" pattern #1141 shipped. A non-positive value
  remains a `ValueError`.
- **Verified by the A4 CI leg.** The `windows-runtime` job (I#198) `gcc`-compiles
  `zaprt.c` (and thus this branch) on MinGW-w64 under both collectors; the A4
  integration harness (`zap_win_a4_test.c`) is extended to connect a loopback
  socket inside a coroutine and set the idle/interval, exercising the ioctl at
  runtime on a real Windows runner. So, unlike the macOS peer-cred follow-up
  (#1156), this is CI-verifiable today.

## Consequences

- Long-lived idle connections on Windows can now have a useful dead-peer window,
  through the same `set_keepalive_idle` / `_interval` names as POSIX.
- On Windows, setting the idle or interval implicitly enables keepalive (the
  ioctl's `onoff = 1`), and setting one keeps the other's last value (default if
  never set); the probe count and the timing getters are unavailable there.
- `net` remains **Experimental** (STABILITY.md §4.4). Completes the Windows side
  of [stdlib/1141-keepalive-interval.md](1141-keepalive-interval.md).
