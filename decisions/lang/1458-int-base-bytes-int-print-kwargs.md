---
title: int(s, base), bytes(int), and print() formatting kwargs
status: Locked
issue: 1458
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `int(s, base)` / `bytes(int)` / `print(sep=, end=, file=, flush=)`

**Status:** Locked. Batch A of the overload-parity epic #1386 (the low-runtime
overloads; `sum(it, start)`, `complex(re, im)`, and `set(iterable)` were already
implemented).

## Decision

### `int(s, base)`
`int(str, base)` parses a string in `base` 2..=36, or `base` 0 to auto-detect a
`0x`/`0o`/`0b` prefix (CPython-style). `int(x)` (1-arg convert/parse) is
unchanged. Surrounding ASCII whitespace and a leading sign are allowed; an
explicit matching prefix is accepted for base 2/8/16. An out-of-range base, an
invalid digit, an empty value, or trailing garbage raises a catchable
`ValueError` (the throwing `zap_str_parse_int_base` kernel).

- **Leading-zero strictness is not enforced:** a non-prefixed base-0 string is
  treated as decimal, so `int("010", 0)` yields `10` here where CPython raises.
  This is an intentional, documented leniency (the strict form adds no value in
  Zap and complicates the parser).
- **Overflow wraps into `int64`** — Zap returns a fixed-width integer, not a
  bigint, matching the existing `int(str)` behaviour.
- sema: one or two positional args; the 2-arg form requires a `str` value and an
  integer base. A keyword argument or wrong arity is `ZAP4025`.

### `bytes(int)`
`bytes(n)` returns `n` zero bytes; a negative count raises a catchable
`ValueError` (the throwing `zap_bytes_zeros` kernel). The existing
`bytes()`/`bytes(str)`/`bytes(list[int])` forms are unchanged. `bigint` is not
accepted as a count (a fixed-width int only).

### `print(*values, sep=" ", end="\n", file=stdout, flush=False)`
Formatting keyword arguments:

- `sep=` (default `" "`) joins the positional values; `end=` (default `"\n"`)
  terminates.
- **`file=` is an `int64` fd** (as `open`/`io` return; default stdout). Output
  goes through the raw `zap_io_write`; to stdout it goes through `zap_print_str`
  (the same buffered `FILE*` the fast `print` path uses), so a mixed
  `print(...)` / `print(..., sep=)` sequence stays correctly ordered.
- **`flush=` is accepted but a no-op** on the raw-fd path (writes are immediate;
  stdout flushes at exit like the rest of `print`). It is evaluated for its side
  effects.
- With **no** keyword options, `print` stays on the existing fast `Inst::Print`
  path (unchanged). Any keyword option switches to the manual build.
- sema: an unknown keyword is `ZAP4023`; a wrong-typed `sep=`/`end=`/`file=`/
  `flush=` is `ZAP4024`.

All three are positional-arg APIs apart from `print`'s named options;
`seam_sigs.rs` is not touched (builtin kwargs are bound in the arm).

## Rationale

- **`file=` as an fd, not a file object.** Zap's io layer is fd-based
  (`open`/`io.open` return `int64`); an fd keeps `print(file=)` consistent with
  it and avoids forcing a `Writer` wrapper. A buffered file-object target is a
  possible follow-up.
- **`flush=` no-op is honest.** The raw-fd path is unbuffered, so there is
  nothing to flush; accepting the argument keeps ported code working without
  pretending to a buffer that is not there. Recorded so it is a known,
  intentional divergence.
- **Base-0 leniency** keeps the parser small; the strict leading-zero rejection
  is the only CPython divergence and is documented, not silent.

## Consequences

- New runtime kernels: `zap_str_parse_int_base` and `zap_bytes_zeros` (gated by
  the LLVM-18 CI job).
- Covered by golden `int_base_bytes_print`, CPython differential cases
  `int_base` / `print_kwargs`, and sema rejection tests in `error_codes`.
- Sibling follow-ups in the epic: `str(bytes, encoding[, errors])` (#1459),
  `list`/`dict` constructors (#1460), deferred `tuple(iterable)` (#1461).
