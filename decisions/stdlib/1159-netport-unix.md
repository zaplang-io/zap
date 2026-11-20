---
title: net.port raises ValueError on a Unix-domain socket
status: Locked
issue: 1159
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# `net.port` on a Unix-domain socket

**Status:** Locked

## Decision

`net.port(sock)` on an AF_UNIX socket (from `net.listen_unix` / `net.connect_unix`,
#993) now raises a catchable `ValueError` ("net: port is undefined for a
Unix-domain socket") instead of returning a meaningless number. AF_INET / AF_INET6
sockets are unchanged — they return the bound port.

`net.port` is now lowered as a throwing call so the raise is catchable in `try`;
it never raises for the TCP/UDP sockets that have a port.

## Rationale

- **It was reading garbage.** `zap_net_port` assumed anything that is not AF_INET6
  is AF_INET and read `sin_port`. On a `sockaddr_un` that byte offset is part of
  the path, so the old result was meaningless, not an error — the worst kind of
  silent wrong answer.
- **Raise, not a sentinel.** A Unix socket genuinely has no port; a `ValueError`
  is unambiguous and matches the module's "wrong-shape input raises" style, where
  a `0`/`-1` sentinel would be just another number a caller could misread.
- **Throwing lowering is required.** A runtime function that can raise a catchable
  exception must be emitted via `throwing_call` (an invoke), or `try`/`except`
  cannot catch it; `net.port` was a plain call, so the lowering flips to throwing.

## Consequences

- Code that (incorrectly) called `net.port` on a Unix socket now gets a clear
  error instead of a bogus number. TCP/UDP callers are unaffected.
- `net` remains **Experimental** (STABILITY.md §4.4). Surfaced by
  [stdlib/993-unix-domain-sockets.md](993-unix-domain-sockets.md).
