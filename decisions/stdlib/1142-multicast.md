---
title: net UDP multicast — join/leave + TTL/loopback, family-dispatched
status: Locked
issue: 1142
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# `net` UDP multicast

**Status:** Locked

## Decision

Add UDP multicast to `net` (follow-up #1142 to #991 UDP / #994 socket options):

- `net.join_multicast(sock, group)` / `net.leave_multicast(sock, group)` — join /
  leave a multicast group on the default interface.
- `net.set_multicast_ttl(sock, hops)` — outgoing multicast TTL / hop limit
  (0..255).
- `net.set_multicast_loopback(sock, on)` — whether this host also receives its
  own outgoing multicast.

Family-dispatched on the socket's `AF_INET` / `AF_INET6` (the #991 explicit-family
model, `udp_socket` vs `udp_socket6`): the group literal must parse in the
socket's family. A malformed group or an out-of-range TTL raises `ValueError`; a
`setsockopt` failure (e.g. no multicast route) raises a catchable error.

## Rationale

- **Named, family-dispatched options.** Membership uses the structured
  `ip_mreq` / `ipv6_mreq` options under the hood, but the surface is the four
  named calls — no raw `IP_ADD_MEMBERSHIP` / level constants, consistent with the
  rest of `net`. The socket already records its family (#991), so one call covers
  both IPv4 (`IP_ADD_MEMBERSHIP` / `IP_MULTICAST_TTL` / `IP_MULTICAST_LOOP`) and
  IPv6 (`IPV6_JOIN_GROUP` / `IPV6_MULTICAST_HOPS` / `IPV6_MULTICAST_LOOP`).
- **The IPv4 scalar optval width.** `IP_MULTICAST_TTL` / `IP_MULTICAST_LOOP` want
  a `u_char` on POSIX (BSD/macOS require it; Linux accepts it) and a `DWORD` on
  Windows; the IPv6 hops/loopback want an `int` everywhere. The runtime sets each
  with the platform-correct width so the options do not silently `EINVAL` on
  macOS.
- **Default interface only, for now.** The common receiver/sender case does not
  pick an interface; an explicit-interface argument (`imr_interface` /
  `ipv6mr_interface`) is a flagged follow-up.
- **Validation up front.** A non-multicast or malformed group address, and a TTL
  outside 0..255, are `ValueError` before the syscall — clearer than a late
  `setsockopt` error.

## Consequences

- UDP multicast receivers/senders (service discovery, telemetry fan-out) are
  expressible in core `net`.
- An explicit-interface selector is a follow-up.
- The golden pins the deterministic parts (the TTL/loopback setters succeed, the
  malformed-group and out-of-range-TTL `ValueError`s) and guards the actual
  join/leave, which needs a multicast route a containerized CI runner may lack.
- `net` remains **Experimental** (STABILITY.md §4.4). Follow-up to
  [stdlib/991-udp-sockets.md](991-udp-sockets.md) and
  [stdlib/994-socket-options.md](994-socket-options.md).
