---
title: Exact scalar min/max built-in free functions
status: Locked
issue: 1279
pr:
date: 2026-11-22
domain: lang
supersedes:
superseded-by:
---

# Exact scalar `min` / `max` (`min(a, b)` / `max(a, b)`)

**Status:** Locked

## Decision

Add two built-in free functions, `min(a, b)` and `max(a, b)`, that return the
lesser / greater of **two numeric scalar arguments**, Python-compatible in
spelling:

```zap
def earlier(int64 attempt_ns, int64 lifetime_ns) -> int64:
    return min(attempt_ns, lifetime_ns)
```

- **Placement:** compiler built-ins (entries in `BUILTINS`, a typed handler in
  `crates/sema`, and lowering in `crates/ir`), not a stdlib seam module.
- **Spelling:** free functions, not methods — matching Python and the call sites
  the DNS library (zaplang-io/dns#6) already writes.
- **Typing:** the result is the numeric join of the operand types
  (`sema_types::num_join`): two signed ints → `int64`, two unsigned ints →
  `uint64`, any float operand → `float64`. A narrower result needs an explicit
  cast, exactly as integer arithmetic does (D-0019/D-0020).
- **Exactness:** when both operands are integers, the comparison is an integer
  `icmp` (signed, or unsigned when either operand is unsigned, #552) and the
  selected value is an integer — **never routed through `f64`**. Adjacent
  `int64` values above 2^53 therefore stay distinct.
- **Tie semantics:** equal operands return the **first** argument (Python
  `min`/`max` semantics). `min` selects the left operand on `l <= r`, `max` on
  `l >= r`.
- **Float operands** compare at `f64` with an ordered predicate, mirroring the
  `<`/`<=` operators.

### Accepted / rejected in this slice

- Accepted: `int32`/`int64`, `uint8`..`uint64`/`usize`, `float32`/`float64`, and
  untyped numeric literals (both operands of a single signedness).
- Rejected (clean diagnostics):
  - more or fewer than two arguments → ZAP4025;
  - any keyword argument → ZAP4023;
  - a non-numeric or non-scalar operand — `str`, `char`, `bool`, `bigint`,
    `complex`, list, … → ZAP4024;
  - mixing a signed and an unsigned operand → ZAP4015 (the same rule the
    comparison operators enforce; no implicit sign conversion).

The arguments are positional-only and take no keywords, so `seam_sigs.rs` is
**not** touched (and `min`/`max` are not seam functions).

## Rationale

- **The exactness is the whole point.** The issue came from a real DNS CI failure
  bounding a per-attempt deadline by the remaining lifetime — both absolute
  monotonic `int64` nanosecond timestamps. Comparing them through `f64` collapses
  neighbours above 2^53. Lowering to `icmp` + `select` keeps the integer domain.
- **Built-in, not stdlib.** The result type must preserve operand width across
  the numeric tower; the positional-only, loosely-typed stdlib seam cannot
  express that, and Python exposes `min`/`max` as built-ins. A built-in also lets
  the lowerer emit a branchless compare + `select` with no runtime call.
- **Reuse the blessed numeric machinery.** Typing uses `num_join`; lowering uses
  the same widening (`coerce` to a common 64-bit width, `is_unsigned`-driven
  signed/unsigned `icmp`) that the comparison operators already use, so min/max
  behaves consistently with `<`/`<=`.
- **Two-arg only, deliberately.** Keeping the first slice to the exact two-arg
  scalar form unblocks the DNS use case with an airtight exactness guarantee and
  a small, exhaustively testable surface.

## Consequences

- Pure-integer `min`/`max` is exact across the full signed range, adjacent
  values above 2^53, equal inputs, and negatives (golden `scalar_minmax`).
- An explicit int/float mix promotes to `float64` — the same `f64` promotion the
  `<`/`<=` operators apply; a caller wanting an exact mixed comparison converts
  explicitly. Pure-integer calls never promote.
- `min`/`max` now shadow any user intent to define free functions of those names,
  consistent with the other built-ins.
- The variadic `min(a, b, c)`, iterable `min(xs)`, `key=`, and `default=` forms,
  int-exact `sum`, and `str`/`char`/`bool`/`bigint` support are **follow-ups**
  (#1284), under the epic #1286 that groups the Python-parity built-in
  free-function work.
