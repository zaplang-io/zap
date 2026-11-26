---
title: Handler adapter binds wider scalar parameter types
status: Locked
issue: 1625
pr:
date: 2026-11-26
domain: lang
supersedes:
superseded-by:
---

# Handler adapter binds wider scalar parameter types

**Status:** Locked

## Decision

The argparse-backed handler adapter (#1544) binds the full set of scalar
parameter types that have a value-conversion builtin, not only
`int64`/`float64`/`bool`/`str`. The argparse `Namespace` exposes four typed
getters — `get_int` (`int64`), `get_float` (`float64`), `get_flag` (`bool`),
`get_str` (`str`) — so a **narrower** scalar reads the wide getter and converts:

| parameter type                          | adapter reads                     |
|-----------------------------------------|-----------------------------------|
| `int`/`int64`/`i64`                     | `ns.get_int("p")`                 |
| `int32`/`i32`                           | `int32(ns.get_int("p"))`          |
| `uint8`/`uint16`/`uint32`/`uint64`/`usize` | `uintN(ns.get_int("p"))`       |
| `float`/`float64`/`f64`                 | `ns.get_float("p")`               |
| `float32`/`f32`                         | `float32(ns.get_float("p"))`      |
| `bool`                                  | `ns.get_flag("p")`                |
| `str`                                   | `ns.get_str("p")`                 |

So a handler `def cmd(int32 n, uint8 level)` now adapts, where it was a
decoration-site error (`ZAP5048`) before.

## Scope

The conversion builtins that exist as scalar value conversions are
`int32`/`int64`, `uint8`/`uint16`/`uint32`/`uint64`/`usize`, and
`float32`/`float64` (`numeric_conv_ty` in the lowerer). `int8`/`int16` have no
scalar conversion builtin, and `char`/`bytes`/`bigint`/`complex` are not
argv-sourced scalars, so all of these remain a precise `ZAP5048` — widening to
them is out of scope.

## Rationale

- **Reuse the existing conversions.** A narrower scalar is bound by wrapping the
  wide getter in the already-supported numeric conversion call, so no new runtime
  getter and no `Namespace` change are needed; the conversion is checked and
  lowered exactly as an explicit `int32(x)` would be.
- **Additive.** The getter-native types (`int64`/`float64`/`bool`/`str`) read
  directly as before; only previously-rejected narrower scalars change from
  `ZAP5048` to a bound argument.

## Consequences

- `crates/driver/src/decorator_desugar.rs`: `namespace_getter_for` returns the
  getter plus an optional conversion builtin; the synthesized adapter wraps the
  getter read in that conversion when present.
- **Deferred:** `int8`/`int16` (no scalar conversion builtin) and
  `char`/`bytes`/`bigint`/`complex`.

<!-- Related: #1544 (the adapter this widens), zaplang-io/cli#4. -->
