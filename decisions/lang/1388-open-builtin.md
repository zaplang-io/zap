---
title: open() prelude built-in as sugar over io.open
status: Locked
issue: 1388
pr:
date: 2026-11-24
domain: lang
supersedes:
superseded-by:
---

# Built-in `open(path[, mode])`

**Status:** Locked

## Decision

Add `open` to the prelude as thin syntactic sugar over the existing
`io.open(path, mode)` (D-0056), for Python familiarity. `input` (the other half
of the original #1388 scope) already shipped (#969, D-0212); this change is just
`open`.

- **`open(path)`** forwards to `io.open(path, "r")` — mode defaults to `"r"`.
- **`open(path, mode)`** forwards to `io.open(path, mode)`.
- **Return type:** the same `int64` file descriptor `io.open` returns — usable
  with `io.read` / `io.write` / `io.close_fd`, and wrappable with `io.reader` /
  `io.writer` / `io.lines`.
- **Failure:** identical to `io.open` — a catchable error on failure (the
  lowering emits the same throwing `zap_io_open`): a missing path raises
  `FileNotFoundError` (catchable as `OSError` / `Exception`), a bad mode raises
  `ValueError` (#1320, D-0247).

### Positional, not keyword

`open` takes its arguments **positionally** (`open(path)` / `open(path, mode)`),
matching `io.open`, which is positional. A keyword argument (`open(p, mode="w")`)
is a `ZAP4023` error. Keeping it positional means `seam_sigs.rs` is **not**
touched; if keyword `mode=` is wanted later it is an additive follow-up (a
`seam_sigs` entry), not a breaking change.

### Typing

- One or two arguments; both must be `str` (ZAP4024 otherwise). Wrong arity is
  ZAP4025; a keyword argument is ZAP4023.
- Result is `int64`.

## Rationale

- **Pure sugar, no new capability.** The file-open capability, the fd handle
  type, and the typed-error behaviour all already exist in `io.open`; `open`
  only defaults the mode and gives it the Python spelling. The lowering reuses
  the already-declared throwing `zap_io_open` — no new runtime kernel, no
  codegen-C change.
- **Same handle, same errors.** Returning the `int64` fd (rather than a new
  file-object type) keeps `open` interchangeable with `io.open` and composes
  with the whole `io` surface, so there is one file model, not two.
- **Positional like its target.** `io.open` is positional; mirroring that avoids
  a `seam_sigs` entry and a divergence between the sugar and the function it
  forwards to.

## Consequences

- `open` now shadows a user free function of that name, like the other
  built-ins.
- `open` returns a bare fd, so `with open(...) as f:` (a context-managed
  file object, as in Python) is **not** provided by `open` itself; the buffered
  `io.writer(fd)` / `io.reader(fd)` wrappers (with `__enter__`/`__exit__`) are
  the object surface. A context-managed `open` returning a file object directly
  is a possible follow-up, not part of this thin alias.
