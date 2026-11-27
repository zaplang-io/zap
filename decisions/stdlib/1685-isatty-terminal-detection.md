---
title: io.isatty(fd) — terminal detection, infallible, false for any non-terminal or invalid descriptor
status: Accepted
issue: 1685
pr:
date: 2026-11-27
domain: stdlib
supersedes:
superseded-by:
---

# `io.isatty(fd)` terminal detection

**Status:** Accepted.

## Decision

Add a terminal-detection query to the raw-fd I/O surface:

```zap
bool interactive = io.isatty(sys.stderr_fd())
```

- Signature `io.isatty(fd: int64) -> bool`. The fd is positional-or-keyword
  (`io.isatty(fd=...)`), matching the rest of the `io` fd surface.
- It is a **pure, infallible query**: it never raises, and it leaves the
  descriptor's file offset and the terminal state unchanged.
- A descriptor connected to a PTY or platform console reads as **true**; a pipe,
  a redirected regular file, a closed descriptor, and an out-of-range /
  otherwise invalid descriptor all read as **false**. This folds `EBADF` into
  `false` exactly as CPython `os.isatty` does, rather than raising.
- Two descriptors in one process answer independently, so a program can tell a
  redirected stream from a console when only one of stdout/stderr is redirected.
- Backed by POSIX `isatty(2)` and Windows `_isatty`, covering the platform
  targets of the core I/O surface (`crates/codegen/src/runtime/os_fs.inc.c`).

## Rationale

The issue specifies the surface closely (`os.isatty(fd)` parity, positional fd,
decide invalid-fd behavior in core). The only genuine choice is the invalid-fd
contract: raise, or return false. **False, never raising** is chosen because:

- it matches CPython `os.isatty`, the stated analogue;
- the sole purpose of the query is "is this a console?", and an invalid or
  non-terminal descriptor is simply "not a console" — forcing callers to wrap
  every `isatty` in `try`/`except` to answer a yes/no question is noise;
- it keeps the lowering a plain (non-throwing) call, so no `invoke`/landing-pad
  machinery is pulled in for a one-word syscall.

A window-size query (`TIOCGWINSZ`) is explicitly separable and is not added
here, as the issue notes.

## Consequences

- Unblocks the CLI progress bar (zaplang-io/cli#12) and rich help
  (zaplang-io/cli#16): `io.isatty(sys.stdout_fd())` distinguishes a console from
  a pipe/file so output can adapt.
- `io.isatty` is infallible; callers never need to handle an exception from it.
  A program that wants to treat a bad descriptor as an error must check validity
  by other means — this query reports only terminal-ness.
- Pairs with [stdlib/1684-getpass-hidden-input](1684-getpass-hidden-input.md),
  the hidden-input primitive that shares this release's terminal work.
