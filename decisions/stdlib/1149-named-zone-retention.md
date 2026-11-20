---
title: stdlib datetime — retain named zones on DateTime and resolve civil times across DST gaps/folds
status: Accepted
issue: 1149
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# stdlib `datetime` — named-zone retention + civil-time resolution (gap/fold)

**Status:** Accepted (maintainer sign-off 2026-11-20). The `int64 zone_id`
layout (§1), the civil resolver (§2), the resolution API (§3) and **pure-Python
wall-clock `add` (§4)** all land in this PR. Calendar units (months/years) stay
out of core and become dateutil `relativedelta` in datetime-utils#2, built on top
of this.

## Problem

Under [[D-0225]] (#1045, shipped PR #1052) an aware `DateTime` carries only
`int64 offset` + `bool is_aware`. `astimezone(dt, zone("America/New_York"))`
binary-searches the bundled IANA table for the instant's offset, attaches that
*materialised* offset, and **drops the zone**. So a later wall-clock operation
re-uses the stale offset:

```zap
auto ny     = datetime.zone("America/New_York")
auto inst   = datetime.rfc3339_parse("2024-03-09T17:00:00Z")   # local noon EST
auto local  = datetime.astimezone(inst, ny)                    # -05:00, correct
datetime.isoformat(datetime.replace(local, day=10))            # keeps -05:00 — WRONG
```

Local noon on Mar 10 is EDT (`-04:00`): `replace` must re-apply the zone's rule,
but the value no longer knows the zone. D-0225 explicitly deferred this
(["Naive-wall-clock-in-zone localization"](../../DECISIONS.md), #1055 "Not in
scope"). There is also no reverse civil→zone operation that classifies a wall
time as unique / gap (imaginary) / overlap (ambiguous) — the dateutil
`datetime_ambiguous` / `datetime_exists` / `resolve_imaginary` / `fold` surface.
This blocks dateutil-compatible wall-clock arithmetic in datetime-utils#2 and
gap/fold classification in datetime-utils#3 (pkg tracking #324).

## Decision

### 1. Zone identity on `DateTime` — an `int64 zone_id` handle (pointer-free)

Add **one `int64 zone_id` field** to the prelude `value struct DateTime`:

```
value struct DateTime:
    int64 year, month, day, hour, minute, second
    int64 offset          # seconds east of UTC (unchanged)
    bool  is_aware         # unchanged
    int64 zone_id          # NEW: named-zone handle
```

Encoding:
- `zone_id == -1` → **no named zone**: a naive value (`is_aware == false`) *or*
  a fixed-offset value (`is_aware == true`, constant `offset`). Behaviour is
  byte-for-byte the pre-#1149 model; `replace`/arith keep `offset` verbatim.
- `zone_id >= 0` → index into the bundled `ZAP_TZ_ZONES[]` name array (sorted by
  name, from `tzdata_generated.inc.c`). The value's `offset` is still the
  materialised offset for its instant; `zone_id` lets wall-clock ops re-resolve.

`zone_id` is a **plain integer index**, so `DateTime` stays an all-scalar,
**non-GC-traced** value struct — no new pointer in the layout, no change to the
GC struct map, no `inst_dest()` rooting hazard. The id is process-local (never
serialised); a manual tzdata regen may renumber it, which is harmless because no
`DateTime` outlives the process image that assigned it.

Threading:
- `datetime.zone(name)` validates and, in addition to returning the `TimeZone`,
  the `astimezone` path resolves `name -> zone_id` via a new runtime
  `zap_tz_zone_id(name) -> int64` (binary search; raises for an unknown zone,
  matching today's `zap_tz_zone_require`). `astimezone(dt, named)` stores that
  id; `astimezone(dt, fixed_offset(...))` stores `zone_id == -1`.
- `replace(dt, …)` on a `zone_id >= 0` value recomputes `offset` for the new
  civil fields via the civil resolver below (`fold = 0`); on `zone_id == -1` it
  keeps `offset` exactly as today.
- All existing constructors (`from_timestamp*`, `now*`, `parse`, `strptime`,
  `rfc3339_parse`, `combine`, `date`/`time`) set `zone_id == -1`. `add`/`diff`:
  see the semantic fork below.

### 2. Civil-time resolution — pure over the existing `(at, off)` tables

A new runtime resolver maps a naive wall clock + zone to instant(s) **using only
the bundled `(transition_utc, offset)` intervals already shipped** — no table
regeneration, no `is_dst` flag needed. Algorithm (robust 2-candidate, matches
CPython `zoneinfo` / PEP 495):

> Let `w = timegm(civil)`. For interval `i` with offset `off_i` spanning UTC
> `[at_i, at_{i+1})`, the wall time `w` is realised by interval `i` iff
> `at_i <= w - off_i < at_{i+1}`. Count matches in the O(1) window around the
> binary-search point: **0 ⇒ gap/imaginary, 1 ⇒ unique, 2 ⇒ overlap**; the two
> overlap instants are `w - off_i` for each matching `i`.

New runtime entries (all take civil components + zone id, `throwing_call` where
they can raise):
- `zap_tz_resolve_local(zone_id, y,mo,d,h,mi,s, fold) -> offset` — the offset to
  store. Unique → that offset; overlap → earlier (`fold=0`) / later (`fold=1`);
  gap → PEP 495 imaginary resolution (the post-transition offset), so the value
  round-trips like CPython.
- `zap_tz_civil_kind(zone_id, y,mo,d,h,mi,s) -> int` — `0` unique, `1` gap,
  `2` overlap. Backs `is_imaginary` (`==1`) and `is_ambiguous` (`==2`).

### 3. Public seam surface (Python `zoneinfo`/`dateutil`-faithful)

- `datetime.resolve(tz, year, month, day, hour, minute, second, fold=0) -> DateTime`
  — *localize*: attach `tz` to a wall clock, picking the `fold` instant on an
  overlap and PEP-495-resolving a gap. Aware result carrying `zone_id`. (The
  `TimeZone` is positional; the civil fields are positional ints; `fold` is an
  optional trailing int — keyword or positional.)
- `datetime.is_ambiguous(dt) -> bool` — dt's wall time occurs twice (fall-back)
  in its **retained** zone.
- `datetime.is_imaginary(dt) -> bool` — dt's wall time never occurs
  (spring-forward). (`exists` = `not is_imaginary`.)
- `datetime.resolve_imaginary(dt) -> DateTime` — shift a gap time forward by the
  gap length (dateutil `resolve_imaginary`); a no-op for a real time.

The classifiers / `resolve_imaginary` take a single `DateTime` and read its own
`zone_id` (dateutil's `datetime_ambiguous` / `datetime_exists` /
`resolve_imaginary` operate on the value's own `tzinfo`); a naive / fixed value
(`zone_id == -1`) is always unique, so both classifiers are false and
`resolve_imaginary` is a no-op. `seam_sigs.rs` rows for the three single-`dt`
facets; `resolve` has a `fold=0` default, so (like `replace`) it binds its own
arguments and is **omitted** from `seam_sigs` (the trailing-optional convention,
D-0204). `seam_kwargs` cases added for the `dt` facets. `fold` is an **input
only** — it is *not* stored on `DateTime` (the chosen `offset` already
distinguishes the two overlap instants), so no extra layout bit.

## Rationale

- **Why a handle on `DateTime`, not a new `ZonedDateTime` type.** The #1149
  acceptance example flows the same value through `astimezone` → `replace`; a
  separate type would break that and double the surface. Python keeps one
  `datetime` with optional `tzinfo`; the handle is the eager analogue.
- **Why `int64 zone_id`, not `str zone`.** A `str` field makes `DateTime` carry
  a GC-managed pointer the precise collector must root — the exact ABI/GC-visible
  change the brief flags, and it adds a heap string + allocation to every aware
  value. The integer index keeps `DateTime` a non-traced POD, reuses the
  name-sorted array already emitted, and is O(1) to the zone's table. We lose
  cheap `%Z` abbreviations and human-inspectable repr of the zone — acceptable
  (D-0225 already renders `%Z` as the numeric `UTC±HH:MM`, and abbreviations were
  separately deferred in #1055).
- **Why resolve from `(at, off)` only.** The committed tables already carry every
  transition instant and offset; the 2-candidate rule derives gap/overlap from
  them exactly. No regen, no new `is_dst` column, no POSIX-rule work at runtime.
- **Why not change the table/`gen_tzdata.py`.** Keeps the build reproducible and
  the generated artifact untouched; the whole feature is compiler + hand-written
  runtime C.

### 4. `add` is pure-Python wall-clock arithmetic (maintainer decision)

`add(dt, delta)` advances the **naive wall-clock fields** by the delta, keeps the
zone, and **re-resolves the offset (fold = 0) at the new naive wall time**. This
is CPython's `datetime + timedelta`: it adds to the wall clock and re-derives the
offset from the zone, it does **not** shift the UTC instant.

> **One rule for every unit.** In Python `timedelta(days=1) ==
> timedelta(seconds=86400)` — both are naive-field additions — so days and
> sub-day units are the *same* operation. A day's worth of seconds therefore
> advances the wall clock one calendar day and re-resolves the offset
> (local noon Mar 9 `−05:00` → Mar 10 `−04:00`); an `add` that lands in a gap or
> an overlap resolves via the civil resolver (§2) with `fold = 0`.

**The representation fork dissolves.** Because days and sub-day are one
naive-field addition, the existing flat `value struct Duration { int64 nanos }`
(D-0227) is **sufficient** — no new delta type, no `days`/`months` field, no
`add_days`/`add_months`/`add_years` primitives, and `Duration`'s "pure nanos /
Go `time.Duration`" identity is intact. `add` lowers to: naive wall epoch
`timegm(civil) + nanos/1e9` → recompose civil → `zap_tz_resolve_local(zone_id,
new_civil, fold=0, old_off)`.

**Backward-compatible.** For a naive / fixed-offset value (`zone_id == -1`),
advancing the naive wall clock equals the old instant-plus-delta result
(`timegm(civil) − off + delta + off == timegm(civil) + delta`) and the re-resolve
returns the stored offset, so those `add` results are byte-for-byte unchanged.
Only zone-bearing values (`zone_id >= 0`) now re-resolve.

**Months / years stay out of core** — they are non-uniform calendar units and
become dateutil `relativedelta` in datetime-utils#2, built on these primitives.

**Gap handling in `replace` / `add`.** When a named-zone value lands in a gap, the
offset resolves to the fold-0 (pre-transition) side (PEP 495) rather than raising;
`resolve_imaginary` is the explicit shift onto the real time.

## Consequences

- New core value layout: every `DateTime` construction site in the lowering sets
  `zone_id`; `build_datetime` gains a `zone_id` param; `replace` re-resolves. The
  `datetime_timezone`/`datetime_strftime` goldens' repr of `DateTime` gains the
  field (naive values show `zone_id=-1`) — a visible but mechanical golden churn.
- Runtime: new C in `time_datetime.inc.c` (same fragment; no new `.inc.c`, so no
  dual-tree-writer / `build.rs` wiring). Resolver calls that can raise go through
  `throwing_call`.
- `STABILITY.md` datetime row note updated (zone retention now real). Grammar
  unchanged (no new syntax). `changelog.d/<PR>.changed.md` (alters aware-value
  behaviour across DST).
- Unblocks datetime-utils#2 (wall-clock arith) and #3 (fold/gap), #324.

Relates to [[D-0225]] (#1045 tz database), #1055 (deferred localization), #1046
(`replace`), and the dateutil tz reference
(<https://dateutil.readthedocs.io/en/stable/tz.html>).
