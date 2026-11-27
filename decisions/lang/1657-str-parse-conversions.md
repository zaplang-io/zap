---
title: str -> bigint and str -> complex scalar conversions
status: Locked
issue: 1657
pr:
date: 2026-11-27
domain: lang
supersedes:
superseded-by:
---

# str -> bigint and str -> complex scalar conversions

**Status:** Locked

## Decision

`bigint(str)` and `complex(str)` parse a string, matching the existing
`int(str)` / `float(str)` surface:

- `bigint(s)` — parses `s` as an arbitrary-precision integer: an optional leading
  sign, an optional `0x`/`0o`/`0b` radix prefix, and `_` digit separators (the
  same grammar as a bigint literal). Beyond the `int64` range `int(str)` is
  limited to; Python's `int("…")` is likewise unbounded.
- `complex(s)` — parses `s` as a Python-style complex literal: `re`, `im j`, or
  `re +/- im j`, with optional surrounding whitespace and one layer of
  parentheses (e.g. `"1+2j"`, `"3j"`, `"-1.5-2e3j"`, `"(1+2j)"`), matching CPython
  `complex("1+2j")`. A string is valid only as the **sole** argument;
  `complex(real, imag)` stays numeric-only.

A malformed string raises a **catchable `ValueError`** (#1566), exactly as
`int(str)` / `float(str)` do.

## Rationale

- **Parity + unblocks the adapter.** Zap's `bigint` is its unbounded integer and
  `complex` its complex number, so string parsing matches Python and the
  obvious `type=`-callable expectation. It unblocks the deferred
  `Namespace.get_bigint` / `get_complex` getters (#1655, epic #1644), whose core
  dependency this was.
- **Reuse the runtime.** `bigint(str)` reuses `zap_bigint_from_str`
  (`zap_big_parse_chars`); its invalid-digit path now raises a typed `ValueError`
  (it was an untyped panic — unreachable from the lexer-validated literal path, so
  no behavior change there). `complex(str)` adds `zap_complex_from_str`, modelled
  on `zap_str_parse_float`.

## Consequences

- `crates/sema/src/lib.rs`: the `bigint`/`complex` conversion typer accepts a
  `str`/`strview` argument (`complex` only as the sole argument).
- `crates/ir/src/lower.rs`: `bigint(str)` lowers to a throwing
  `zap_bigint_from_str`; `complex(str)` (one argument) to a throwing
  `zap_complex_from_str` — both `invoke`, so the `ValueError` is catchable.
- `crates/codegen/src/zaprt.c`: new `zap_complex_from_str`; the bigint parser's
  malformed-input panic becomes `ZAP_EXC_VALUE_ERROR`.
  `crates/codegen/src/lib.rs`: declares `zap_complex_from_str`.
- Empty / no-digit strings for `bigint` currently yield `0` (the existing parser's
  behavior); tightening that is out of scope.

<!-- Related: #1644 (epic), #1655 (the getters this unblocks), #1566 (catchable
errors), #1386 (int base parse). -->
