---
title: stdlib datetime — resolve a wall clock with both a DST fold and a fractional second
status: Accepted
issue: 1361
pr:
date: 2026-11-24
domain: stdlib
supersedes:
superseded-by:
---

# stdlib `datetime` — `resolve(tz, …, fold=, nanosecond=)`

**Status:** Accepted. Additive, keyword-only `nanosecond=` selector on
`datetime.resolve`. `datetime.replace`'s documented default fold is unchanged.

## Problem

The `python-dateutil` parser (non-core datetime-utils#4) distinguishes the two
occurrences of a repeated wall time — e.g. `2024-11-03 01:30:00.123456 EST` in
`America/New_York` — using the zone abbreviation (#1345), and must keep *both*
the chosen occurrence and the fractional second in one `DateTime`. Today neither
core entry point can build that value:

- `datetime.resolve(tz, y, mo, d, h, mi, s, fold=)` (#1149) selects the fold
  instant but writes a hard-coded `0` to `F_NANOSECOND` — whole seconds only.
- `datetime.replace(dt, nanosecond=…)` (#1148) carries a fraction but
  **re-resolves the offset at a constant `fold = 0`** ([[1149-named-zone-retention]],
  the PEP-495 default). Applying it to a fold-1 value silently rewrites the
  offset back to the earlier (fold-0) instant.

So `resolve(…, fold=1)` then `replace(…, nanosecond=…)` loses the fold, and
there is no single operation that produces *chosen fold + fraction*.

## Decision

### 1. Add the fraction to `resolve`, not to `replace`

Give `datetime.resolve` a keyword-only `nanosecond=` selector. `resolve` already
takes an explicit `fold`, so a chosen overlap instant and a sub-second fraction
are constructed together in one call:

```zap
auto ny = datetime.zone("America/New_York")
auto late = datetime.resolve(ny, 2024, 11, 3, 1, 30, 0, fold=1, nanosecond=123456789)
# fold=1 -> EST, offset -18000; fold=0 -> EDT, offset -14400; both keep all nine digits
assert late.offset == -18000
assert late.nanosecond == 123456789
```

**`replace`'s default fold stays 0.** The issue explicitly prefers this:
`replace` documents `fold = 0` for wall-clock replacement ([[1149-named-zone-retention]]),
and changing it to preserve the source instant's fold would be a behavioural
break for every existing `replace` caller and is unnecessary once `resolve`
expresses the combination directly. `resolve` is the civil-time *constructor*,
so the fraction belongs on it alongside `fold`.

### 2. Surface — `nanosecond=` is keyword-only

`resolve(tz, year, month, day, hour, minute, second, fold=0, nanosecond=0)`:

- The TimeZone is positional first, then the six positional civil ints.
- `fold` keeps its existing spelling: keyword `fold=` **or** an 8th positional
  (back-compat with #1149 callers).
- `nanosecond` is **keyword-only** (`nanosecond=N`). A 9th positional is
  rejected, so the new fraction can never be confused with the civil fields or a
  positional fold. The two keyword selectors may appear in either order.
- Default `nanosecond=0` reproduces the historical whole-second behaviour, so
  every existing `resolve` call is byte-for-byte unchanged.

### 3. Lenient storage — no range validation

The fraction is stored verbatim in `F_NANOSECOND`, exactly like `datetime.replace`
and like `resolve`'s civil fields (which are themselves un-validated, matching the
lenient `parse` family). An out-of-range value (`>= 1_000_000_000`) is kept in the
field; `isoformat` / `rfc3339` render a fraction only for `0 < nanos < 1e9` (an
out-of-range fraction is omitted from the rendered string, the pre-existing
renderer policy). The in-range boundaries `0` and `999_999_999` behave as
expected. This keeps `resolve` consistent with the rest of the module rather than
introducing a bespoke raising path.

### 4. Seam binding — no `seam_sigs.rs` row

`resolve` already binds its own arguments (it is intentionally **omitted** from
`crates/syntax/src/seam_sigs.rs`) because its optional trailing `fold` is rejected
by the trailing-only seam binder ([[D-0204]]). The keyword-only `nanosecond=` is
another optional selector, so the bespoke binding in `crates/sema/src/modules/datetime.rs`
is extended and **no seam signature is added**. The workspace rule (update
`seam_sigs.rs` *unless the arguments stay positional / self-bound*) is satisfied:
`resolve` self-binds, and `seam_kwargs` cases cover the new keyword.

## Alternatives rejected

- **Change `replace`'s default fold to preserve the source fold.** Breaks the
  documented `replace` contract (#1149) and every existing caller; unnecessary
  once `resolve` supports the combination. Rejected (the issue says so).
- **A positional 9th `nanosecond`.** Ambiguous next to the optional positional
  fold and the six civil fields; keyword-only is unambiguous and matches the
  issue's spelling. Rejected.
- **Validate the range and raise on out-of-range.** Diverges from `replace` and
  from `resolve`'s own lenient civil fields; would be the only raising civil
  constructor. Rejected for consistency.

## Consequences

- One additive keyword on an existing function; whole-second `resolve` /
  `replace` results are unchanged.
- `resolve` can now construct chosen-fold + fractional values in one call,
  unblocking the datetime-utils#4 parser's repeated-wall-time handling.
- `resolve_imaginary` already carries the fraction through a gap shift (#1148),
  so a fractional gap value round-trips.
