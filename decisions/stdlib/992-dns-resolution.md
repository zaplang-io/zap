---
title: net DNS resolution surface — lookup_host / lookup_ipv4 / lookup_ipv6 / lookup_addr
status: Locked
issue: 992
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# `net` DNS resolution surface

**Status:** Locked

## Decision

Add an explicit name-resolution surface to the core `net` module (second
sub-issue of epic #985). Until now `net.connect` resolved internally and code
could not resolve a name itself. The surface is string-based `list[str]`,
matching Go's `net.LookupHost` / `LookupIP` / `LookupAddr` and Python's
`socket.getaddrinfo` / `gethostbyname`:

- `net.lookup_host(name) -> list[str]` — all addresses (A + AAAA), as numeric IP
  strings.
- `net.lookup_ipv4(name) -> list[str]` — IPv4 (A) addresses only.
- `net.lookup_ipv6(name) -> list[str]` — IPv6 (AAAA) addresses only.
- `net.lookup_addr(ip) -> list[str]` — reverse (PTR) lookup of a numeric IP to
  its hostname(s); an empty list when the address has no reverse record, and a
  `ValueError` when `ip` is not a numeric address.

All resolution runs on `getaddrinfo` / `getnameinfo` bracketed by
`enter/leave_blocking` (the scheduler treats the coroutine as blocked during a
real DNS query; numeric inputs resolve with no network). A resolve failure
raises a catchable error.

## Rationale

- **String `list[str]`, not an opaque addrinfo handle.** The practical need —
  pre-resolve/cache, pick a family, health-check by IP, custom connection
  strategies — is served by a list of numeric IP strings that the existing
  `net.connect(ip, port)` / `net.connect_ip_deadline(ip, port, deadline_ns)` and
  `tls.connect_resolved` consume directly (they already accept numeric
  literals). An opaque `addrinfo` struct type would add machinery for no gain,
  and plain strings are trivially cacheable and comparable. So "getaddrinfo-style
  resolution feeding connect_resolved" (the issue's second bullet) is realized as
  `lookup_host` + the existing numeric connects, not a new handle type.
- **Explicit family variants (`lookup_ipv4` / `lookup_ipv6`) instead of a family
  flag argument.** Zap exposes no `AF_*` / `AI_*` constants; dedicated variants
  give the issue's "address-family selection" in the same explicit-family style
  as #991's `udp_socket` / `udp_socket6`, with no flag-constant surface. The
  all-families default is `lookup_host`.
- **`ai_socktype = SOCK_STREAM`** in the forward resolver so each address appears
  once, not once per socket type (getaddrinfo otherwise multiplies entries).
- **Reverse lookup returns an empty list for a missing PTR, not an error.** "This
  IP has no reverse record" is a normal outcome, distinct from a malformed
  address (`ValueError`); returning `[]` lets callers branch without a `try`.
  getnameinfo yields one canonical name, so the list carries at most one entry —
  the `list[str]` shape still matches Go's `LookupAddr` (which can carry several).
- **No service/port name resolution** (`getaddrinfo(host, "http")`). Port numbers
  in Zap are plain integers; a `getservbyname`-style surface is low value and is
  deferred. Flagged, not guessed.

## Consequences

- Core `net` can now pre-resolve, cache, select a family, and reverse-resolve;
  the remaining epic #985 sub-issues (#993 Unix sockets, #994 socket options) are
  independent follow-ups.
- Goldens assert the network-free deterministic cases (numeric-literal forward
  resolution, the malformed-IP `ValueError`, a loopback reverse that must not
  raise) and guard the IPv6 numeric case so it stays deterministic where a
  container lacks AF_INET6.
- `net` remains **Experimental** (STABILITY.md §4.4). Builds on the UDP sockets
  of [stdlib/991-udp-sockets.md](991-udp-sockets.md).
