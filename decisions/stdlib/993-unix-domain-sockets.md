---
title: net Unix domain sockets — AF_UNIX stream (listen_unix/connect_unix) + datagram
status: Locked
issue: 993
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# `net` Unix domain sockets (AF_UNIX)

**Status:** Locked

## Decision

Add AF_UNIX sockets to the core `net` module (third sub-issue of epic #985) for
local IPC over a filesystem socket path — the transport Docker, systemd, local
databases and agent daemons speak. Matches Python's `socket` (`AF_UNIX`) and
Go's `net` (`"unix"` / `"unixgram"`).

**Stream** (the primary surface):

- `net.listen_unix(path) -> listener` — bind + listen an AF_UNIX `SOCK_STREAM`
  socket at `path`.
- `net.connect_unix(path) -> conn` — dial an AF_UNIX stream socket.

Both reuse the TCP server/client surface unchanged: `net.accept`, `net.recv`,
`net.send`, `net.close_conn`, `net.close_listener`, `net.set_read_timeout`,
`net.set_read_deadline`.

**Datagram** (SOCK_DGRAM, the issue's optional part — included to fully close
#993):

- `net.unixgram_socket() -> sock` — unbound AF_UNIX datagram socket.
- `net.unixgram_bind(sock, path)` — bind a local path so it can receive.
- `net.sendto_unix(sock, data, path) -> int64` — send one datagram to a path.
- `net.recvfrom_unix(sock, n) -> (data, path)` — receive one datagram and the
  sender's bound path (`""` for an unbound/abstract sender).

Everything runs non-blocking on the netpoller, parking on a would-block, exactly
like the INET transports; the handle is the same `ZapSock` (`family = AF_UNIX`).

## Rationale

- **Stream reuses the TCP surface verbatim.** An AF_UNIX stream connection is
  byte-stream semantics identical to TCP; only the bind/connect address differs
  (a path, not host+port). So only the two constructors are new, and
  `accept`/`recv`/`send`/`close_*`/deadlines carry over — the smallest surface
  that works, and the one Python/Go users expect.
- **Datagram mirrors the #991 UDP paths, addressing by path.** `unixgram_socket`
  / `unixgram_bind` / `sendto_unix` / `recvfrom_unix` are the exact UDP shapes
  with a path in place of host+port; `recvfrom_unix` returns `(data, path)`
  (a 2-tuple — no port for AF_UNIX) built the same way as UDP's `(data, host,
  port)`. Included rather than deferred so #993 closes fully; it is low-risk
  given the UDP template.
- **Pathname sockets only; POSIX only.** Linux abstract-namespace addresses
  (leading-NUL `sun_path`) are rejected as an invalid path for now — portable,
  unambiguous, and the overwhelmingly common case. AF_UNIX `SOCK_DGRAM` does not
  exist on Windows and `SOCK_STREAM` AF_UNIX there carries caveats, so the
  Windows build raises a clear "not supported" error.
- **No stale-path auto-unlink.** `listen_unix` fails with the OS error if `path`
  already exists (a stale socket file), rather than silently unlinking a
  caller-named path — same contract as Python `bind` and Go `ListenUnix` (which
  unlinks only on close). Remove a stale path first.
- **Peer credentials deferred.** `SO_PEERCRED` / `LOCAL_PEERCRED` is genuinely
  optional, platform-divergent, and orthogonal; filed as a follow-up rather than
  guessed here.

## Consequences

- Local-IPC clients/servers over a socket file are now expressible in core
  `net`; the last epic #985 sub-issue (#994 socket options) is independent.
- Goldens cover a stream echo round-trip (two coroutines over a `fs.mkdtemp`
  path) and a datagram round-trip with a deterministic sender-path check; both
  POSIX-only (CI's Linux legs).
- `net` remains **Experimental** (STABILITY.md §4.4). Builds on
  [stdlib/991-udp-sockets.md](991-udp-sockets.md) and
  [stdlib/992-dns-resolution.md](992-dns-resolution.md).
