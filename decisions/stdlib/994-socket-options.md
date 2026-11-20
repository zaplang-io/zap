---
title: net socket options — named set_*/get_* for reuse, nodelay, keepalive, buffers
status: Locked
issue: 994
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# `net` socket options

**Status:** Locked

## Decision

Add the common socket options to the core `net` module (fourth and final
sub-issue of epic #985) as **named setters and getters**, in the same
explicit-name style as the rest of `net` — Zap exposes no `SOL_SOCKET` / `SO_*`
/ level or option constants, so there is no generic
`setsockopt(level, optname, value)`. Matches the intent of Python's
`socket.setsockopt` / `getsockopt` and Go's `SetNoDelay` / `SetKeepAlive` /
`SetReadBuffer` / `SetWriteBuffer`:

Setters (value is a `bool` for the on/off options, an `int64` byte count for the
buffers):

- `net.set_reuseaddr(sock, on)` — `SO_REUSEADDR` (prompt listener rebind).
- `net.set_reuseport(sock, on)` — `SO_REUSEPORT` (load-balance across listeners;
  raises where the platform lacks it).
- `net.set_nodelay(sock, on)` — `TCP_NODELAY` (disable Nagle, low latency).
- `net.set_keepalive(sock, on)` — `SO_KEEPALIVE` (idle-connection health).
- `net.set_recv_buffer(sock, nbytes)` — `SO_RCVBUF`.
- `net.set_send_buffer(sock, nbytes)` — `SO_SNDBUF`.

Getters:

- `net.get_nodelay(sock) -> bool`, `net.get_keepalive(sock) -> bool`.
- `net.get_recv_buffer(sock) -> int64`, `net.get_send_buffer(sock) -> int64`
  (the OS value, which may be rounded up or doubled from what was set).

A `setsockopt` / `getsockopt` failure (e.g. a TCP-level option on a non-TCP
socket, or `SO_REUSEPORT` where unsupported) raises a catchable error. Set
address-reuse options on an unbound socket (e.g. `net.udp_socket`) before the
matching `*_bind`; `TCP_NODELAY` and the buffer sizes apply to an established
connection.

## Rationale

- **Named options, not a generic `setsockopt(level, opt, val)`.** Consistent with
  every other `net` entry (and with #991/#992/#993 in this epic), which never
  surface raw OS constants. The named form is self-documenting, type-checked
  (bool vs byte count), and portable (the runtime maps each to the right
  `level`/`optname`).
- **Both setter and getter for each.** Python/Go let you read an option back; the
  getters are cheap and make the surface symmetric. Buffer getters matter in
  particular because the kernel rounds/doubles the requested size, so the
  effective value differs from what was set.
- **Portable, not POSIX-gated.** `setsockopt`/`getsockopt` and the `SO_*` /
  `TCP_NODELAY` names exist on Winsock too, so these compile and work on all
  platforms — only `SO_REUSEPORT` is `#ifdef`-guarded and raises where absent.
- **Keepalive interval and multicast membership deferred.** Per-platform keepalive
  tuning (`TCP_KEEPIDLE`/`TCP_KEEPINTVL`/`TCP_KEEPCNT` on Linux vs
  `TCP_KEEPALIVE` on macOS) and multicast join/leave are platform-divergent and
  lower priority; `set_keepalive` covers the on/off the issue centers on, and the
  rest is flagged as a follow-up rather than guessed.

## Consequences

- Servers can rebind promptly (`SO_REUSEADDR`/`SO_REUSEPORT`), latency-sensitive
  clients can disable Nagle, idle connections can enable keepalive, and buffer
  sizes are tunable — closing epic #985's last sub-issue. The epic
  (#991 UDP, #992 DNS, #993 Unix, #994 options) is complete.
- A golden pins the deterministic behavior (boolean options read back exactly;
  buffer sizes asserted positive; reuse options applied before an unbound bind).
- `net` remains **Experimental** (STABILITY.md §4.4). Builds on
  [stdlib/991-udp-sockets.md](991-udp-sockets.md),
  [stdlib/992-dns-resolution.md](992-dns-resolution.md), and
  [stdlib/993-unix-domain-sockets.md](993-unix-domain-sockets.md).
