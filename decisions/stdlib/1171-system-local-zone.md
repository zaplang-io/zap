---
title: stdlib datetime — expose the system local zone as a reusable TimeZone
status: Accepted
issue: 1171
pr:
date: 2026-11-20
domain: stdlib
supersedes:
superseded-by:
---

# stdlib `datetime` — `local_zone() -> TimeZone`

**Status:** Accepted. `datetime.local_zone()` returns the host's
system-configured zone as a **named** `TimeZone` (an IANA bundle handle), so it
re-resolves correctly across DST in `astimezone` / `resolve`, unlike a one-shot
offset snapshot. Resolution is Unix-oriented (`TZ`, `/etc/localtime`,
`/etc/timezone`); an unresolvable host zone raises a catchable error.

## Problem

Core had `now_local()` / `from_timestamp_local(ts)`, which attach the host offset
for **one instant** and set `zone_id = -1` (no reusable handle). `TimeZone`
handles came only from `zone(name)` (an explicit bundled IANA key) or
`fixed_offset(minutes)`. There was no core way to obtain a `TimeZone` for the
*system* zone, which blocks `dateutil.tz.tzlocal()` / `gettz()` / `gettz(None)`
in `zaplang-io/datetime-utils#3` (tracking #324). A package-level snapshot of the
host's current UTC offset would be wrong for other instants across DST, and
re-implementing host-zone discovery in the package would duplicate core.

## Decision

`datetime.local_zone() -> TimeZone` (no arguments). It resolves the host zone to
a bundled IANA **name** and returns `TimeZone{ name, offset = 0, fixed = false }`
— identical in shape to `zone(name)`, so the resulting handle carries a real
`zone_id` through `astimezone`/`resolve` and re-applies the zone's transition
rules at every instant.

**Platforms & configuration sources (Unix), in order:**

1. **`TZ` environment variable**, when set and non-empty: a leading `:` is
   stripped; if the remainder is a bundled IANA name it is used. (A POSIX rule
   string like `EST5EDT` is not an IANA name and falls through.)
2. **`/etc/localtime` symlink target**: the path suffix after the last
   `zoneinfo/` component (e.g. `/usr/share/zoneinfo/America/New_York` →
   `America/New_York`), when it is a bundled name.
3. **`/etc/timezone`** (Debian/Ubuntu): the first trimmed line, when it is a
   bundled name.

**Failure (documented):** if none of the sources yields a name present in the
bundled IANA data — including on platforms without these files (e.g. Windows,
unless `TZ` names a bundled zone) — `local_zone()` raises the **same catchable
error** as an unknown `zone(name)` (`unknown time zone: …` / "cannot determine
system local zone"). Callers that want a non-throwing fallback can `try` it and
fall back to `fixed_offset(0)` / `zone("UTC")`.

The resolution + bundle validation live in one runtime entry
`zap_datetime_local_zone_name() -> ZapStr*` (throwing — raises on an
unresolvable/absent zone), invoked via `throwing_call`. Because it returns an
already-validated bundled name, `astimezone`/`resolve` on the handle never raise.

## Rationale

- **A named handle, not an offset snapshot** — the acceptance explicitly requires
  the same handle to yield `-05:00` in January and `-04:00` in July for
  `America/New_York`, with the bundled spring gap / fall overlap. Only a
  `zone_id`-carrying `TimeZone` (re-resolved per instant, #1149) does that.
- **Reuse `zone(name)`'s shape** — `local_zone()` is "discover the name, then be
  `zone(name)`", so it needs no new `TimeZone` representation and inherits all of
  `astimezone`/`resolve`'s behaviour for free.
- **Unix sources, raise elsewhere** — the bundled tz data and the rest of the tz
  runtime (`localtime_r`, the `(at,off)` tables) are already Unix-oriented, and
  dateutil's `tzlocal` is itself platform-dependent. Honoring `TZ` on every
  platform plus the two standard Unix files covers the CI/test matrix and
  Linux/macOS hosts; a Windows CLDR→IANA mapping is out of scope and documented
  as raising. This is the one judgment call in this ADR.

## Consequences

- One new rowed-less 0-arg facet `local_zone` (sema arity-0 like `now`/
  `date_today`, so **no** `seam_sigs` row; a `seam_kwargs` arity case is added).
  One new runtime entry in the existing `time_datetime.inc.c` fragment (no new
  `.inc.c`, so no dual-tree-writer / `build.rs` wiring); it is throwing.
- `STABILITY.md` datetime row note updated (system local zone now available).
- `local_zone()` is **host-dependent**, so goldens cover it only by driving it
  under a fixed `TZ` (deterministic); the offset-across-DST behaviour is covered
  by the existing bundled-zone goldens via the returned handle.
- Unblocks `datetime-utils#3` (`tzlocal`/`gettz`) and #324.

Relates to `decisions/stdlib/1149-named-zone-retention.md` (the `zone_id`
handle), D-0225 (the aware `DateTime` + tz database), and #324.
