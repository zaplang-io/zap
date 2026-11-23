---
title: Typed OSError family across the os/fs/io surface (generalizing #1311)
status: Locked
issue: 1320
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# Typed `OSError` family across the `os` / `fs` / `io` surface

**Status:** Locked

## Decision

#1311 (decisions/lang/1311-filesystem-notfound-error.md) gave `fs.read_file` /
`fs.read_bytes` a typed `FileNotFoundError` (ENOENT) vs generic `OSError`, with
`FileNotFoundError ⊂ OSError ⊂ Exception`. This generalizes that typing across
the rest of the `os` / `fs` / `io` / `process` runtime surface
(`crates/codegen/src/runtime/os_fs.inc.c`), which previously raised a single
**untyped** `Exception` (via `zap_sys_fail` → `zap_panic`) for every failure.
Every raise site is now classified by the failure it reports:

- **errno-driven OS/filesystem failures → `OSError`, with `ENOENT` →
  `FileNotFoundError`** (`zap_sys_fail_errno(op, errno)`, generalizing #1311's
  `zap_fs_read_fail`). Covers `io.open`/`read`/`write`/`close`, `fs.size`/`stat`/
  `rename`/`move`/`copy`/`makedirs`/`rmtree`/`copytree`/`scandir`/`walk`/
  `read_file`/`read_bytes`/`write_file`/`write_bytes`/`append`, `os.listdir`/
  `chdir`/`remove`/`rmdir`/`mkdir`/`getcwd`/`setenv`/`path_abspath`/
  `path_realpath`, and the `process` spawn paths (so a **missing executable**,
  `posix_spawn`/exec → `ENOENT`, raises `FileNotFoundError`, matching CPython).
- **allocation failures → `MemoryError`** (`zap_sys_oom(op)`), first-class since
  #882 (decisions/lang/882-memoryerror-exception.md), never a bare `Exception`.
- **caller-input validation → `ValueError`**: `io.open` invalid mode, an empty
  path to `fs.makedirs`, an empty `process` argument list.
- **OS-capability / environment failures → `OSError`**: an over-long temp path,
  a temp-file create failure, a rename-onto-target failure, an unresolvable
  realpath, a `CreateProcess` failure, a closed/absent subprocess stdin, and the
  `process.Popen` "not supported on this platform" family.

`process.run` / `check_run` keep raising `RuntimeError` for a **non-zero child
exit** (that is a process-result condition, not an OS error); `input()` keeps
`EOFError` at end-of-input; a `process` timeout keeps `TimeoutError`. Every new
type descends from `Exception`, so this is **additive**: an existing `except
Exception` still catches every failure; only code that wants the finer
distinction opts into the new names.

### Companion fix: `read_file` / `read_bytes` on a directory

`zap_read_all_fd` trusted `lseek(SEEK_END)` as its initial-allocation size hint.
For a directory (and pipes / character devices) that value is unreliable and
drove an out-of-memory abort before the subsequent `read` could surface the real
error. A read now `fstat`s the fd and trusts the `lseek` size **only for a
regular file**, falling back to the 4096-byte growth path otherwise — so a
directory read reaches the `EISDIR` → `OSError` path cleanly instead of OOMing.
(This predates #1311; its generic raise had the same bug, which is why #1311's
golden could not assert a real directory read. #1320's `os_fs_typed_errors`
golden now does.)

## Rationale

- **Removes the same ambiguity #1311 removed, everywhere else.** The DNS use
  case (zaplang-io/dns#7) needed only `read_file`, but `fs.stat`/`size`,
  `os.listdir`, `io.open`, `fs.scandir`/`walk`, `fs.rename`/`copy`/`move` all
  pose the identical "is it missing or a real error?" question to a caller.
- **CPython parity, no new types.** `OSError`, `FileNotFoundError`,
  `MemoryError`, `ValueError`, `RuntimeError`, `EOFError`, `TimeoutError` already
  exist end-to-end; this is a pure retyping of raise sites plus the one
  directory-read fix. A missing executable raising `FileNotFoundError` matches
  CPython's `subprocess`.
- **One shared, errno-safe helper.** `zap_sys_fail_errno` captures the errno at
  the syscall (so an intervening allocation cannot clobber it) and `zap_sys_oom`
  / `zap_sys_fail_typed` keep the `"<op>: <detail>"` message format identical, so
  uncaught-report text and any message-matching callers are unaffected — only the
  exception *type* is now specific. The untyped `zap_sys_fail` is retained (other
  runtime fragments still use it).

## Scope and consequences

- **Finer `OSError` errno subtree not added.** Zap still distinguishes only the
  missing-path case (`FileNotFoundError`) from the generic `OSError`; CPython's
  `PermissionError` / `IsADirectoryError` / `FileExistsError` / `NotADirectoryError`
  are a separate future decision. Match on `OSError` for the non-missing cases.
- **No `NotImplementedError` type.** Platform "not supported" failures raise
  `OSError` (CPython raises `OSError`/`NotImplementedError` here; Zap uses
  `OSError` until a dedicated type is justified).
- **Pre-existing non-throwing lowerings unchanged.** `fs.glob` and `io.reader`
  lower via a plain (non-`invoke`) call, so a raise from their runtime backing is
  not catchable — the same as before this change (a typing change does not alter
  catchability). Flipping them to `throwing_call` is a separate, orthogonal fix.
- **Behaviour change (additive).** Specific `os`/`fs`/`io` failures now raise
  `OSError` / `FileNotFoundError` / `MemoryError` / `ValueError` instead of the
  base `Exception`; all remain subclasses of `Exception`, so `except Exception`
  handlers are unaffected.

See also D-1311 (the read-path precedent), #882 (`MemoryError`), and the
downstream need in zaplang-io/dns#7.
