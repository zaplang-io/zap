---
title: str(bytes, encoding[, errors]) explicit decode
status: Locked
issue: 1459
pr:
date: 2026-11-25
domain: lang
supersedes:
superseded-by:
---

# `str(bytes, encoding[, errors])`

**Status:** Locked. Batch B of the overload-parity epic #1386.

## Decision

Add the explicit-decode forms of `str` for a `bytes` operand. The 1-argument
`str(bytes)` stays UTF-8 / strict (I#337); `str(x)` for every other type is the
unchanged Display render.

- **`str(bytes, encoding)`** and **`str(bytes, encoding, errors)`** decode the
  bytes to a `str`.
- **Encodings** (case-insensitive): `utf-8` (`utf8`), `ascii` (`us-ascii`),
  `latin-1` (`latin1` / `iso-8859-1` / `l1`). An unknown encoding raises a
  catchable `ValueError`.
- **errors**: `strict` (default) raises a catchable `ValueError` on invalid
  input; `replace` substitutes `U+FFFD`; `ignore` drops the offending
  byte/subpart. An unknown errors handler raises `ValueError`. `latin-1` never
  fails, so its `errors` argument is immaterial.
- Only the `bytes` operand takes the extra arguments; a non-`bytes` first
  argument with a second argument is a `ZAP4024` type error. One to three
  positional arguments (`ZAP4025` otherwise); a keyword argument is `ZAP4025`.

## Divergence from CPython (intentional, documented)

CPython raises **`UnicodeDecodeError`** for a bad strict decode and
**`LookupError`** for an unknown encoding/errors handler. Zap has neither in its
exception hierarchy, so both raise **`ValueError`** (the catch-all for
caller-input errors, consistent with `int()`/`bytes()` and D-0247). A program
that must distinguish them would need those exception types added first — a
separate decision. Because of this, the error cases are covered by the golden
suite, not the CPython differential corpus; the successful decodes (round-trip,
`replace`/`ignore` counts, `latin-1` mapping) are differential-tested and match
CPython exactly.

## Implementation

- sema `str` arm: 1..=3 positional args; the 2-/3-arg form requires a `bytes`
  value and `str` encoding/errors.
- Lowering: `str(bytes, enc[, errs])` -> the throwing `zap_str_decode`, with
  `errs` defaulting to a `"strict"` string constant.
- Runtime (`zap_str_decode` dispatch): reuses the existing `zap_bytes_to_str`
  path idea (UTF-8 strict via `zap_utf8_valid`) and `zap_utf8_decode_replace`;
  adds `zap_utf8_decode_ignore`, `zap_ascii_decode` (strict/replace/ignore), and
  `zap_latin1_decode`.

## Consequences

- One new runtime entry point `zap_str_decode` (+ the static decoders), gated by
  the LLVM-18 CI job.
- Covered by golden `str_decode`, CPython differential `str_decode` (successful
  cases), and sema rejection tests `str_decode_codes`.
- Sibling follow-ups in the epic: `list`/`dict` constructors (#1460), deferred
  `tuple(iterable)` (#1461). `int(base)`/`bytes(int)`/`print` kwargs shipped in
  #1458.
