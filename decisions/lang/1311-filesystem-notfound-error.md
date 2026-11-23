---
title: FileNotFoundError (⊂ OSError) raised by filesystem reads on a missing path
status: Locked
issue: 1311
pr:
date: 2026-11-23
domain: lang
supersedes:
superseded-by:
---

# `FileNotFoundError` for a missing path on a filesystem read

**Status:** Locked

## Decision

Introduce two first-class built-in exception types and raise them from the
filesystem read surface (`fs.read_file`, `fs.read_bytes`):

- **`OSError`** (`ZAP_EXC_OS_ERROR = 19`, parent `Exception`) — the generic
  operating-system / I/O error base. CPython parity: `OSError` sits directly
  under `Exception`, and `IOError` is a legacy alias of it.
- **`FileNotFoundError`** (`ZAP_EXC_FILE_NOT_FOUND_ERROR = 20`, parent
  `OSError`) — raised when a read's `open(2)` fails with `ENOENT`. CPython
  parity: `FileNotFoundError` ⊂ `OSError` ⊂ `Exception`.

A filesystem read maps the failing `errno` at the point of the syscall:

| Failure                                            | Type                 |
|----------------------------------------------------|----------------------|
| path does not exist (`open` → `ENOENT`)            | `FileNotFoundError`  |
| any other open failure (permission, is-a-directory,`EACCES`/`EISDIR`/…) | `OSError` |
| post-open read error (`read` → non-`EINTR` error)  | `OSError`            |
| successful read                                    | returns the contents |

Both new types descend from `Exception`, so an existing `except Exception`
continues to catch every read failure unchanged. The message string is
unchanged (`"fs.read_file: <strerror>"`), so the uncaught-report text and any
message-matching callers are unaffected — only the exception *type* is now
specific.

```zap
import fs

def load_config() -> str:
    try:
        return fs.read_file("/etc/resolv.conf")
    except FileNotFoundError:
        return default_resolv_conf()      # absent file → defaults
    # a permission / directory / other I/O error is NOT caught here:
    # it propagates as an OSError and surfaces to the caller.
```

`except OSError` catches both a missing file and any other read failure;
`except FileNotFoundError` catches only the missing-file case; `except
Exception` catches everything.

## Rationale

- **Spelling: `FileNotFoundError`, matching CPython.** Zap's north star is
  Python-readability, and the issue explicitly asks for the Zap spelling of
  Python's `FileNotFoundError` / Go's `errors.Is(err, fs.ErrNotExist)`. Python's
  name reads correctly and needs no translation. This is the smallest surface
  that satisfies the acceptance criteria without a new `errno`-inspection API.

- **A real `OSError` base, not `FileNotFoundError` directly under `Exception`.**
  The hierarchy had no OS/IO error base. Parenting `FileNotFoundError` straight
  under `Exception` would satisfy the missing-vs-other distinction (the "other"
  bucket being the generic `Exception`), but the issue asks that non-ENOENT
  failures "still raise — a more generic `OSError`/`IOError`". A typed `OSError`
  gives those failures a named home, lets `except OSError` catch the whole
  filesystem-error family while `except FileNotFoundError` selects just the
  missing-path case, and reproduces CPython's exact tree so the mental model
  transfers. It is also the natural umbrella for the rest of the `OSError` errno
  subtree (`PermissionError`, `IsADirectoryError`, `FileExistsError`, …) when
  those are added, so we never have to re-parent `FileNotFoundError` later.

- **`errno` captured at the syscall.** `zap_fs_read_fail(op, err)` takes the
  errno by value, captured immediately after the failing `open`/`read`, so an
  intervening allocation inside the message build cannot clobber `errno` before
  the type is chosen.

- **Mirror an existing type, touch every wiring site.** The two new ids are
  added at exactly the sites `MemoryError` (#882, D-0158) occupies — the
  `ZAP_EXC_*` enum and `zap_exception_parent` in `zaprt.c`, sema's builtin name
  table and `is_exception_type`, lowering's `exception_type_id_name`, and the
  pyinterop name→id map — so no exhaustive match (host or LLVM-only) breaks and
  the compile-time and runtime name resolutions agree. The read lowerings
  already use `throwing_call` (invoke), so a typed raise unwinds into an
  enclosing `try/except` exactly as the previous generic raise did; no lowering
  change is needed.

## Scope and consequences

- **Scoped to filesystem reads (#1311).** Only `fs.read_file` and
  `fs.read_bytes` classify their errno. The mutating / metadata fs surface
  (`fs.write_file`, `fs.append`, `fs.mkdir`, `fs.remove`, `fs.rename`,
  `fs.stat*`, …) still raises the generic `zap_sys_fail` → `Exception`. Giving
  those the same `OSError` / typed-errno treatment (and adding the broader
  `PermissionError` / `IsADirectoryError` / `FileExistsError` subtree) is a
  worthwhile consistency follow-up, filed separately rather than widening this
  issue.

- **Pre-existing: `read_file` / `read_bytes` on a directory does not raise
  cleanly.** On POSIX `open(dir, O_RDONLY)` succeeds, so the failure is only
  discovered by the subsequent `read` (`EISDIR`); the size hint
  `zap_read_all_fd` takes from `lseek(SEEK_END)` on a directory is unreliable and
  can drive an out-of-memory abort before that `read` runs, so the typed
  `OSError` is not reached for the directory case on those platforms (on Windows
  `open` fails `EACCES` up front and the typed `OSError` *is* raised). This
  read-buffer fragility predates #1311 — the previous generic raise had the same
  problem — and fixing it (e.g. an `fstat`/`S_ISREG` guard before trusting the
  `lseek` size, or detecting the directory up front) is tracked as a follow-up
  rather than widened into this error-typing change. Because of it, the
  "non-missing failure raises `OSError`, not `FileNotFoundError`" distinction is
  pinned by the `fs_notfound_hierarchy` golden (user `raise`s, deterministic on
  every platform) instead of by a real directory read.

- **No `IOError` alias name in Zap source.** CPython keeps `IOError` only as a
  legacy alias; Zap source spells it `OSError`. A Python-interop exception named
  `IOError` still maps to `ZAP_EXC_OS_ERROR` (so caught Python I/O errors behave
  correctly), but `except IOError` is not a Zap spelling.

- **Behaviour change (additive, non-breaking for `except Exception`).** A
  missing-file read now raises `FileNotFoundError` and a permission/other read
  failure now raises `OSError` instead of the base `Exception`. Both remain
  subclasses of `Exception`, so existing `except Exception` handlers are
  unaffected; only code that wants the finer distinction opts into the new
  names. `raise FileNotFoundError(...)` / `raise OSError(...)` and the matching
  `except` clauses are now available to user code.

See also D-0158 (`MemoryError`, `decisions/lang/882-memoryerror-exception.md`),
the `TimeoutError` (I#485) / `EOFError` (#1036, D-0215) wiring precedents, and
the downstream need in `zaplang-io/dns#7` (loading `resolv.conf`).
