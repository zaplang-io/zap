---
title: stdlib datetime — subsecond precision on core DateTime (nanoseconds-of-second)
status: Accepted
issue: 1148
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# stdlib `datetime` — subsecond precision on the core `DateTime`

**Status:** Accepted. Core `DateTime` gains one `int64 nanosecond` field
(nanoseconds-of-second, `0..=999_999_999`); `add`/`diff` carry the sub-second
remainder instead of truncating the `Duration` to whole seconds; `replace` gains
`microsecond=` / `nanosecond=` keywords; and the ISO/RFC-3339 renderer
(`isoformat`/`rfc3339`) and the ISO parser (`parse`/`rfc3339_parse`) round-trip a
fractional component.

## Problem

Core `DateTime` (`[[../../DECISIONS.md]]` D-0225, reshaped by the #1149 named-zone
work) stopped at whole seconds. `datetime.add(dt, delta)` divided the
nanosecond-backed `Duration` ([[1047]] / D-0227) by 1_000_000_000 and threw the
remainder away, so `add(start, time.micros(1))` compared **equal** to `start` and
the formatter had no fractional field to emit. `replace` had no subsecond field,
and the ISO/RFC-3339 parser + formatter could neither read nor write a fraction.
This blocked dateutil-compatible microsecond deltas in
`zaplang-io/datetime-utils#2` (package tracking #324).

## Decision

### 1. Representation — one `int64 nanosecond` field on `DateTime`

The prelude `value struct DateTime` gains a tenth field:

```
value struct DateTime:
    int64 year, month, day, hour, minute, second
    int64 offset          # seconds east of UTC (D-0225)
    bool  is_aware        # D-0225
    int64 zone_id         # named-zone handle, -1 = none (#1149)
    int64 nanosecond      # NEW: nanoseconds-of-second, 0..=999_999_999
```

- **Sub-decision (a) — precision/width: nanoseconds-of-second, stored `int64`.**
  Nanoseconds (not microseconds) match `Duration`'s nanosecond base, so `add`
  carries the remainder with **no rounding** and `diff` is exact to the
  nanosecond; the field satisfies "at least microsecond" with three decimal
  digits of headroom. The width is `int64` rather than the `int32` the brief
  floated, purely because every prelude `value struct` field is `int64` and the
  `DateTime` POD is already all-`int64` (9 scalars); a narrow field would be the
  only `int32` in the layout for no benefit (`0..=999_999_999` fits either).
- A **plain integer** keeps `DateTime` an all-scalar, **non-GC-traced** POD — no
  new pointer, no change to the GC struct map, no `inst_dest()` rooting hazard
  (identical reasoning to #1149's `zone_id`).
- **`StructNew` zero-initializes**, so the field defaults to `0` wherever a
  constructor does not set it (every whole-second constructor, the `@derive(Json)`
  decode path `gen_dt_from_epoch`, and `combine`). Whole-second construction is
  therefore **byte-for-byte unchanged** in behaviour.

### 2. `add` / `diff` stop truncating

`add(dt, delta)` keeps the #1149 pure-wall-clock model but combines the fraction
first: `total = dt.nanosecond + delta.nanos`, `carry = floor(total / 1e9)`,
`new_nanosecond = total − carry·1e9` (always `0..=999_999_999`). The whole-second
`carry` advances the naive wall clock (so carry/borrow ripples through second →
minute → hour → **day/month/year** via the existing `timegm`/`gmtime` recompose),
the offset is re-resolved at the new wall time (`fold=0`, #1149), and
`new_nanosecond` is stored. Two pure runtime helpers do the **floored** split
(`zap_datetime_nanos_carry`, `zap_datetime_nanos_of_second`) because IR `Div`/`Rem`
truncate toward zero, which borrows wrongly for a negative delta (−1 µs at
`00:00:00.000000`).

`diff(a, b) -> Duration` returns `(instant(a) − instant(b))·1e9 + (a.nanosecond −
b.nanosecond)` ns, so an ordinary microsecond-scale difference is observed.

Comparisons (`before`/`after`/`equal`) compare the pair **(instant, nanosecond)
lexicographically** — not a single `instant·1e9 + nanos`, which would overflow
`int64` outside ≈1678–2262. So two values in the same second but different
fractions order correctly without narrowing the supported date range.

`Duration` itself is **unchanged** (D-0227): `add` just reads `delta.nanos`
without dividing. `to_timestamp` still returns whole `int64` seconds (it cannot
carry a fraction; unchanged).

### 3. `replace` — `microsecond=` and `nanosecond=`

`replace` (bespoke keyword binder, D-0226, not a seam row) accepts two new
optional keywords in addition to the six civil fields:

- `microsecond=N` → `nanosecond = N · 1000` (Python-faithful, the acceptance
  surface);
- `nanosecond=N` → `nanosecond = N` (full-precision escape hatch).

At most **one** of the two may appear in a call (both → "multiple values for the
fractional component", the same error shape as a duplicated field). When neither
is given, the base value's `nanosecond` **rides through unchanged** (like the
zone). For a named-zone value the offset is still re-resolved against the new wall
clock (#1149); the fraction does not affect offset resolution.

### 4. Formatter policy and parser (sub-decision (b))

**Formatter (`isoformat` / `rfc3339`).** The fraction is emitted **iff
non-zero**, placed between the seconds and the offset suffix. A fraction that is
a **whole number of microseconds** renders as **6 digits** — exactly Python
`isoformat`, trailing zeros included (`.250000`, `.000001`, `.100000`); a finer
value renders as **9 digits** (nanoseconds, `.123456789`); a zero fraction is
**omitted** (Python). RFC-3339 accepts this variable-length fraction. This makes
the renderer byte-identical to CPython `datetime.isoformat()` for every value
representable at microsecond resolution (the common case, and the datetime-utils
surface), while still rendering the three extra nanosecond digits exactly when
present — a strict superset of Python, not a divergence.

The **basic** `datetime.format(dt)` / `format_iso` and `strftime` stay
whole-second (no `%f` in our directive subset) — unchanged, so the AWS/JSON ISO
paths that reuse `format_iso` do not churn. Adding `%f` to `strftime` is a
possible follow-up, not required here.

**Parser (`parse` / `rfc3339_parse`).** A new runtime `zap_datetime_parse_nanos`
reads an optional `.`/`,` fraction after the seconds in the time portion and
returns nanoseconds-of-second: it accepts a **general variable-length fraction of
1–9 digits**, right-padding to nanoseconds (`.5` → 500_000_000); digits beyond 9
are consumed but truncated. No fraction → `0`. Lenient, matching the `parse`
family's v1 contract. `rfc3339_parse` scans only after the `T`/space separator so
the date hyphens are never mistaken for a sign, mirroring the offset scanner.

## Rationale

- **Nanoseconds, not microseconds** — exact `Duration` carry, exact `diff`, and
  future-proof; the only cost (field read exposes `nanosecond`, not Python's
  `microsecond`) is bridged in the package layer (`.microsecond =
  nanosecond // 1000`) and by `replace(microsecond=)`.
- **Field, not a new type** — the acceptance value must *remain a core
  `DateTime`* with the fraction intact as it flows through `add`/`replace`/
  `astimezone`/`isoformat`; a sidecar type would break that, exactly as #1149
  rejected a separate `ZonedDateTime`.
- **Runtime helper for the floored split** — the one correctness trap is
  borrow at a second boundary for a negative delta; IR `Div`/`Rem` truncate
  toward zero, so the split is done in C where floor is explicit and unit-tested.
- **Lexicographic comparison** — observes the fraction without the overflow a
  `nanos-since-epoch` int64 would hit for dates outside ≈1678–2262.

## Consequences

- Every `DateTime` constructor in the lowering sets (or zero-inits) `nanosecond`;
  `add`/`astimezone`/`resolve_imaginary` carry the source fraction, `combine`/the
  whole-second constructors leave it `0`. `build_datetime` gains a `nanos` param.
- `isoformat`'s runtime signature gains a trailing `nanos` arg (`declare_rt`
  updated); three new pure runtime entries (`zap_datetime_nanos_carry`,
  `zap_datetime_nanos_of_second`, `zap_datetime_parse_nanos`) in the existing
  `time_datetime.inc.c` fragment — **no** new `.inc.c`, so no dual-tree-writer /
  `build.rs` wiring. No throwing call is added (the parser is lenient, not
  raising).
- No golden prints the `DateTime` repr, so whole-second goldens are unaffected;
  new goldens cover the fraction. `STABILITY.md` datetime row note updated.
- Unblocks `datetime-utils#2` (dateutil `relativedelta` microseconds) and #324.

Relates to [[1047]] (`Duration`, unchanged here), D-0226 (`replace`), D-0225 +
`decisions/stdlib/1149-named-zone-retention.md` (the layout this extends), and
#324.
