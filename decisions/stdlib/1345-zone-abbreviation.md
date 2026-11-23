---
title: stdlib datetime — expose the IANA zone abbreviation at an instant
status: Accepted
issue: 1345
pr:
date: 2026-11-23
domain: stdlib
supersedes:
superseded-by:
---

# stdlib `datetime` — `zone_abbreviation(dt) -> Optional[str]`

**Status:** Accepted (maintainer sign-off 2026-11-23). Additive accessor; the
tz-database bundle is regenerated to carry a per-transition abbreviation; numeric
abbreviations are returned faithfully. `strftime` / `%Z` are unchanged.

## Problem

`python-dateutil`'s `parse(..., tzinfos=...)` (non-core datetime-utils#4) selects
the occurrence of a repeated wall time whose abbreviation matches the parsed
input: `2024-11-03 01:30 EDT` → the `-04:00` instant, `… EST` → the `-05:00`
instant. That needs the IANA abbreviation (`EDT`/`EST`) of a named zone at a
given instant. Core can resolve both instants (#1149) but exposes no abbreviation:

- `strftime`'s `%Z` deliberately formats the numeric offset (`UTC-04:00`), by
  [[D-0225]] (#1045) — that stays.
- An offset alone does not establish an abbreviation (`-04:00` is EDT, AST, VET,
  CLT, …), so a new accessor is required, not a reinterpretation of `%Z`.

## The data gap

The issue assumed the abbreviation "comes from the tz-database transitions
already vendored in core." It does not. `gen_tzdata.py` parses host
`zdump -v` output — which *does* print `EST`/`EDT` — but the compiled bundle
`tzdata_generated.inc.c` stored only `struct { int64 at; int32 off; }` per
transition; the abbreviation column was dropped. Abbreviations are arbitrary tz
strings that cannot be derived from an offset, so the bundle must be regenerated
to carry them.

## Decision

### 1. API — `datetime.zone_abbreviation(DateTime dt) -> Optional[str]`

Free-function style, matching the rest of the module (`datetime.strftime(dt,…)`),
rather than Python's method `dt.tzname()`. Returns the IANA abbreviation of
`dt`'s retained named zone at `dt`'s instant, else `Optional.None`:

- **Named zone (`zone_id >= 0`):** `Some(abbr)`. Determined by the stored UTC
  instant, so it is identical whether `dt` was produced by `resolve(…, fold=)` or
  by converting from UTC. The `America/New_York` fall-back overlap reports
  `Some("EDT")` (fold 0) then `Some("EST")` (fold 1).
- **Naive, fixed-offset, or unknown zone:** `None`. All three carry
  `zone_id == -1` (a fixed offset does not establish an abbreviation — the issue
  says so explicitly), so one rule (`zone_id < 0 → None`) covers them.

The single `DateTime` argument is intentionally positional, so per the workspace
seam rule **no `crates/syntax/src/seam_sigs.rs` entry is added** and no
`seam_kwargs` case is needed.

### 2. Faithful abbreviations (including numeric forms and `LMT`)

Return exactly what the tz database records, matching CPython
`zoneinfo.tzname()`: alphabetic names (`EST`, `CET`, `CEST`), numeric forms the
modern database uses for many zones (`-04`, `+0530`, `+0330`), and `LMT` for the
pre-standard interval. Filtering these would diverge from CPython and lose
information dateutil's match-by-abbreviation can use.

### 3. Bundle layout — an abbreviation index that fits existing padding

`gen_tzdata.py` is extended to capture the abbreviation, intern the unique
abbreviations into an order-stable global pool `ZAP_TZ_ABBRS[]`, and store an
`int32` index per transition:

```
typedef struct { int64_t at; int32_t off; int32_t abbr; } zap_tz_trans;
```

The pre-existing `{int64 at; int32 off}` was already padded to 16 bytes for
8-byte alignment, so the `int32 abbr` fills that padding — **the 38k-entry
transition array does not grow**; only the small pool (a few hundred short
strings) is added. A new interval now begins when *either* the offset or the
abbreviation changes, so an abbreviation-only transition (same offset, renamed
zone) is recorded; the extra split leaves the offset at every instant unchanged.
The dedup key includes the abbreviation, so two zones with identical offset
histories but different names are no longer merged. Regeneration stays offline
and manual via host `zdump` (a committed artifact, not run in CI), unchanged from
D-0225; the host tz tooling was verified byte-identical before regenerating, so
offsets at every instant are preserved.

### 4. Runtime + lowering

- `zap_tz_abbr_at_id(int64 zone_id, int64 inst) -> void*` (in the existing
  `runtime/time_datetime.inc.c` fragment): the abbreviation of the last interval
  with `at <= inst` as a fresh `ZapStr*`, or `NULL` for `zone_id < 0`.
  Non-raising; reuses the offset binary search.
- `Optional[str]` has the niche layout (`Some(p)` *is* the pointer, `None` *is*
  NULL), so the nullable result is already the physical Optional value — lowering
  only reinterprets it to the enum's ZIR type. Monomorphization encodes the
  concrete `Optional$str` instance into the call symbol
  (`zone_abbreviation$Optional$str`), mirroring the `dict.get` → `get$Optional$str`
  precedent.

## Alternatives rejected

- **Return `str` with an empty sentinel for "no abbreviation."** Un-idiomatic for
  a language with a real `Optional`, and a workaround for the Optional-return
  plumbing; rejected.
- **Map numeric/`LMT` forms to `None`.** Diverges from CPython and drops data;
  rejected (see §2).
- **Store a `const char *abbr` pointer per transition.** Adds 8 bytes × 38k
  transitions to the binary; the `int32` index into a pool costs nothing in the
  array (fills existing padding). Rejected.
- **Change `%Z` to emit the abbreviation.** Breaking and explicitly out of scope
  (D-0225); this accessor is additive.

## Consequences

- One additive public function; `strftime`/`%Z` output is byte-for-byte
  unchanged (asserted by a regression test).
- The vendored tz bundle gains an abbreviation pool (a few KB); the transition
  array is unchanged in size.
- Unblocks datetime-utils#4 (`tzinfos=` abbreviation resolution).
