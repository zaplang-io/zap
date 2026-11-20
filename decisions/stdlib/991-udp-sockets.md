---
title: net UDP datagram sockets — udp_socket / bind / connect / sendto / recvfrom
status: Locked
issue: 991
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# `net` UDP datagram sockets

**Status:** Locked

## Decision

Add connectionless UDP (`SOCK_DGRAM`) sockets to the core `net` module, over the
same opaque `bytes` handle and the same netpoller the TCP streams use (#104).
First sub-issue of epic #985 (net transports & options). The surface mirrors
Python's `socket` (`SOCK_DGRAM`) / Go's `net` datagram API, adapted to Zap's
module-function shape (the socket is the first argument — Zap has no methods on
the opaque handle):

- `net.udp_socket() -> sock` — an unbound IPv4 (`AF_INET`) datagram socket.
- `net.udp_socket6() -> sock` — an unbound IPv6 (`AF_INET6`) datagram socket.
- `net.udp_bind(sock, host, port)` — bind a local address; `port 0` asks the OS
  for an ephemeral port (read back with the existing `net.port(sock)`), an empty
  host is the wildcard. `SO_REUSEADDR` is set, mirroring `net.listen`.
- `net.udp_connect(sock, host, port)` — record a default peer so the ordinary
  `net.send` / `net.recv` work on the datagram socket (connected UDP). No
  handshake, so it does not park.
- `net.sendto(sock, data, host, port) -> int64` — send one datagram; returns the
  bytes sent (the connectionless counterpart of `net.send`).
- `net.recvfrom(sock, n) -> (data, host, port)` — receive one datagram (payload
  up to `n` bytes, excess discarded) and the numeric sender IP + port (the
  connectionless counterpart of `net.recv`).

Everything else is **reused unchanged**: `net.send` / `net.recv` (connected UDP),
`net.port`, `net.set_read_timeout`, `net.set_read_deadline`,
`net.set_deadline_ns`, and `net.close_conn`. The handle is the same `ZapSock`; a
trailing `int family` field (AF_INET / AF_INET6) is added so bind / connect /
sendto resolve a target constrained to the socket's family (the aliased leading
`kind` tag and the read-deadline field offsets the TLS seam reads are unchanged).

`recvfrom` returns its `(str, str, int64)` tuple via the same runtime aggregate
path `process.run` uses for its `ZapProc -> (int64, str, str)`: one throwing
runtime call returns a precise GC `ZapDatagram` handle, and three infallible
getters are unpacked into the tuple by the lowerer.

## Rationale

- **Two-step (socket + bind/connect), not a combined constructor.** The existing
  TCP side combines socket+bind+listen (`net.listen`) and socket+connect
  (`net.connect`) because a stream endpoint does exactly one of those. A datagram
  socket is multi-modal — the *same* socket can `bind`, `sendto` many peers,
  `connect` a default peer, and `recvfrom` — so the Python two-step model
  (`socket()` then `bind`/`connect`/`sendto`) fits the transport and the issue's
  named `net.udp_socket()`. Combined `udp_bind(host, port) -> sock` constructors
  were rejected as a worse fit for the connectionless multi-peer case.
- **Explicit v4 / v6 constructors instead of a dual-stack socket.** A single
  `AF_INET6` + `IPV6_V6ONLY=0` dual-stack socket "just works" for both families
  but drags in v4-mapped (`::ffff:a.b.c.d`) literal handling and per-platform
  dual-stack quirks (Windows differs), which make loopback goldens flaky.
  `udp_socket()` (v4) + `udp_socket6()` (v6) is predictable and testable and
  matches Python's own explicit-family model (`socket(AF_INET|AF_INET6, …)`). The
  issue named only `udp_socket()`; the v6 companion is the additive, documented
  extension.
- **Payload is `str`, not Python `bytes`.** The existing `net.recv` / `net.send`
  already carry payloads as `str` (the one `ZapStr` object). UDP stays consistent
  with the module it joins rather than importing Python's `bytes` return for
  datagrams. See `docs/PYTHON_DIFFERENCES.md`.
- **First-argument name `sock`.** A datagram socket is neither a `conn` nor a
  `listener` (the two names the TCP seam rows use); `sock` reads clearest.
- **Single-coroutine loopback is sufficient for tests.** Unlike TCP `accept`,
  a UDP `sendto` to a bound loopback socket is delivered to the destination's
  receive buffer synchronously, so a `recvfrom` finds the datagram waiting — no
  spawn/handshake needed, so the goldens are simple and deterministic.

## Consequences

- Core `net` now covers UDP; the metrics/statsd, DNS-over-UDP, and game/telemetry
  cases the issue lists are unblocked. The remaining epic #985 sub-issues (#992
  DNS resolve, #993 Unix sockets, #994 socket options) are independent follow-ups.
- The `family`-constrained resolution means binding/sending a v6 literal on a v4
  socket (or vice versa) raises a catchable error rather than silently
  misbehaving — the Python contract.
- A dedicated IPv6-loopback golden is guarded by `try`/`except` so it stays
  deterministic where containerized CI runners disable `::1`, while still
  reddening on a genuine v6 round-trip bug; the v6 and v4 runtime paths are
  otherwise one family-parameterized body, pinned directly by the v4 goldens.
- `net` remains **Experimental** (STABILITY.md §4.4).
