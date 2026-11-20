---
title: net AF_UNIX peer credentials — peer_cred (uid/gid/pid)
status: Locked
issue: 1143
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# `net` AF_UNIX peer credentials

**Status:** Locked

## Decision

Add `net.peer_cred(sock) -> (uid, gid, pid)` (follow-up #1143 to #993, which
deferred peer credentials): read the connecting peer's user id, group id and
process id off a connected Unix-domain socket — the authorization pattern a local
daemon uses to check which local user/process is talking to it. Returns an
`(int64, int64, int64)` tuple.

The socket must be an AF_UNIX socket, verified via `getsockname` so it works on
both a `connect_unix` client and an `accept`ed server connection (the accepted
handle's cached family is `AF_UNSPEC`); a non-AF_UNIX socket raises `ValueError`.
**Linux only** for now, via `SO_PEERCRED`; other platforms raise a catchable
"not supported" error.

## Rationale

- **The server-side authorization primitive #993 left out.** A local socket
  server routinely needs to know who connected; `SO_PEERCRED` captures the peer's
  credentials at connect time, which `peer_cred` surfaces as a flat tuple.
- **`getsockname` family check, not the cached handle family.** `net.accept`
  wraps the accepted fd with family `AF_UNSPEC` (it is the shared TCP accept), so
  gating on the handle's stored family would wrongly reject the server side —
  exactly where peer credentials matter most. Asking the OS (`getsockname`)
  reflects the real socket on both ends.
- **Own `zap_ucred` struct, not glibc's `struct ucred`.** glibc hides
  `struct ucred` unless `_GNU_SOURCE` is defined, which this runtime deliberately
  does not define (see the `memmem` note in the net runtime). The `SO_PEERCRED`
  constant is available regardless, so the runtime reads into a locally-declared
  struct matching the kernel's `{pid, uid, gid}` ABI.
- **Linux-first.** macOS/BSD expose this differently (`getpeereid` for uid/gid,
  `LOCAL_PEERPID` for pid) and Windows AF_UNIX has no equivalent. Those paths are
  untestable here (the macOS CI runner is currently parked, #224), so rather than
  ship an unverifiable branch, non-Linux raises a clear "not supported" and the
  macOS/BSD implementation is a flagged follow-up.

## Consequences

- AF_UNIX servers on Linux can authenticate peers by uid/gid/pid.
- macOS/BSD `peer_cred` (`getpeereid` + `LOCAL_PEERPID`) is a follow-up, to land
  when the macOS CI leg is available to verify it.
- The golden exercises a single-process loopback where the peer pid equals
  `os.getpid()`, plus the non-AF_UNIX `ValueError`.
- `net` remains **Experimental** (STABILITY.md §4.4). Follow-up to
  [stdlib/993-unix-domain-sockets.md](993-unix-domain-sockets.md).
