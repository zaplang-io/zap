---
title: Namespace getters for char/bytes and their handler-adapter binding
status: Locked
issue: 1655
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Namespace getters for char/bytes and their handler-adapter binding

**Status:** Locked

## Decision

The argparse object-model `Namespace` (the embedded-Zap prelude struct,
`crates/syntax/src/parser.rs`) gains two non-numeric typed getters, and the
argparse-backed handler adapter (#1544/#1625) binds a handler's `char` / `bytes`
parameters through them:

| getter          | returns | parse rule                                                   |
|-----------------|---------|--------------------------------------------------------------|
| `get_bytes(n)`  | `bytes` | the raw argument string's **UTF-8 encoding** (`bytes(str)`)  |
| `get_char(n)`   | `char`  | **exactly one Unicode codepoint**; `len != 1` raises a catchable `ValueError` |
| `get_bigint(n)` | `bigint`| `bigint(str)` — arbitrary-precision parse (added once #1657 landed) |
| `get_complex(n)`| `complex`| `complex(str)` — Python-style literal parse (added once #1657 landed) |

So a handler `def show(char c, bytes b)` now adapts, where it was a
decoration-site error (`ZAP5048`) before:

```zap
@cli.command
def show(char c, bytes b) -> str:   # => show$impl(ns.get_char("c"), ns.get_bytes("b"))
    ...
```

Unlike the narrowing numeric scalars of #1625 (which read a wide getter and wrap
in a conversion builtin), these getters return the final type directly, so
`namespace_getter_for` maps `char`/`bytes` to the getter with **no** conversion
wrapper.

## Scope

This is sub-issue #1655 of epic #1644. `char`/`bytes` landed first; `bigint`/
`complex` were added once their `str`-parse core dependency (**#1657**) landed —
`get_bigint`/`get_complex` are the same one-line `bigint(self._vals[name])` /
`complex(self._vals[name])` shape, so no workaround was needed. Still **not** bound
by the adapter:

- `int8`/`int16` — not scalar value types in Zap (array dtypes only); a first-class
  signed-narrow scalar family is sub-issue **#1654**, handled separately.

## Rationale

- **Pure Zap, no new runtime.** Both getters are one-liners over the prelude
  `Namespace`'s stored `dict[str, str]`: `get_bytes` = `bytes(self._vals[name])`,
  `get_char` validates `len == 1` (codepoints, #1638) then indexes `v[0]` (a
  `char`, #709). No runtime C, reentrant like the rest of the object model.
- **Python-aligned parse.** `bytes` as the argument's UTF-8 encoding and `char`
  as a single codepoint match the obvious CPython `type=` equivalents; a malformed
  `char` raises a catchable `ValueError` (#1566), consistent with `get_int` /
  `get_float`.
- **Positional args.** The getters take a single positional `name`, like the
  existing `get_str`/`get_int`/`get_float`/`get_flag` object-model getters (no
  `seam_sigs` entry — those cover only the flat `argparse.*` shim).

## Consequences

- `crates/syntax/src/parser.rs`: `Namespace` gains `get_bytes` and `get_char`.
- `crates/driver/src/decorator_desugar.rs`: `namespace_getter_for` maps
  `char` → `get_char`, `bytes` → `get_bytes` (no conversion); the `ZAP5048`
  message lists `char`/`bytes` as supported.
- Partially resolves epic #1644 (the `char`/`bytes` arm); `int8`/`int16` (#1654)
  and `bigint`/`complex` (blocked on #1657) remain.

<!-- Related: #1644 (epic), #1625 (numeric wide scalars), #1657 (str-parse core
dep), #1566 (catchable errors), #709 (char), #1638 (codepoint len). -->
