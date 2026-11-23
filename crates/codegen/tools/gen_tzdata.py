#!/usr/bin/env python3
"""Generate the bundled IANA timezone tables (#1045, D-0225).

Zap bundles the IANA time-zone database so binaries behave identically on every
target (Linux, macOS, musl, Windows), with no dependency on a host
`/usr/share/zoneinfo`. This script emits a flat, pre-computed table of UTC
offset transitions per zone into

    crates/codegen/src/runtime/tzdata_generated.inc.c

which the runtime (`time_datetime.inc.c`) binary-searches at execution time — so
*no* TZif or POSIX-TZ parser runs in the shipped runtime; all of that parsing is
done here, once, offline (D-0225).

Source of truth. Each zone's transitions come from the host `zdump -v <zone>`
(the system's own tz engine, which already decodes the TZif file and expands the
POSIX footer rule into the future). Future transitions are capped at year 2100.
For a fixed-offset zone with no transitions, the constant offset is taken from
Python's `zoneinfo`.

Validation (`--validate`, run offline, NOT in CI). Every emitted interval's
offset is cross-checked at its midpoint instant against Python's `zoneinfo`
(CPython's reference implementation — what Zap's datetime surface is aimed at).
The committed file was generated with 0 mismatches across all available zones.

Reproducing / refreshing the bundle (manual, deliberate — keeps the build
tzdata-free and reproducible):

    python3 crates/codegen/tools/gen_tzdata.py \
        > crates/codegen/src/runtime/tzdata_generated.inc.c
    python3 crates/codegen/tools/gen_tzdata.py --validate >/dev/null   # audit

The generated header records the tzdata version it was built from.
"""

import calendar
import re
import subprocess
import sys
import zoneinfo
from datetime import datetime

# zdump line: "<zone>  Www Mmm DD HH:MM:SS YYYY UT = <local> ABBR isdst=N gmtoff=S"
_GMTOFF = re.compile(r"=\s.*\bgmtoff=(-?\d+)\b")
_UTCIV = re.compile(
    r"^\S+\s+\w+\s+(\w+)\s+(\d+)\s+(\d+):(\d+):(\d+)\s+(-?\d+)\s+UT\s+="
)
# The zone abbreviation is the single token right before `isdst=` (e.g. `EST`,
# `EDT`, `CET`, or a numeric form such as `-04` / `+0530`, or `LMT`). #1345.
_ABBR = re.compile(r"\s(\S+)\s+isdst=")
_MON = {
    m: i + 1
    for i, m in enumerate(
        "Jan Feb Mar Apr May Jun Jul Aug Sep Oct Nov Dec".split()
    )
}
_NEG = -(2**63)  # INT64_MIN — the pre-first-transition (−∞) interval start.
_CAP = calendar.timegm((2101, 1, 1, 0, 0, 0, 0, 0, 0))  # future expansion cap.
_YEAR1 = -62135596800  # 0001-01-01T00:00:00Z — zoneinfo's lower bound.


def intervals(zone):
    """Return [(start_epoch, offset_seconds, abbr)], sorted, start[0] == INT64_MIN.

    A new interval begins when *either* the offset or the abbreviation changes
    (#1345): abbreviation-only transitions — a zone that renames its zone but
    keeps the same offset — must still be recorded so `zone_abbreviation` reports
    the right name, and the extra split leaves the offset at every instant
    unchanged (both halves carry the same offset).
    """
    out = subprocess.run(
        ["zdump", "-v", zone], capture_output=True, text=True
    ).stdout
    ivals = []
    prev = None  # (offset, abbr)
    for line in out.splitlines():
        mo = _GMTOFF.search(line)
        mc = _UTCIV.match(line)
        if not mo or not mc:
            continue
        off = int(mo.group(1))
        ma = _ABBR.search(line)
        abbr = ma.group(1) if ma else ""
        mon = _MON.get(mc.group(1))
        if mon is None:
            continue
        day, hh, mm, ss, yr = (int(mc.group(i)) for i in (2, 3, 4, 5, 6))
        if yr < 1 or yr > 9999:
            continue
        try:
            ep = calendar.timegm((yr, mon, day, hh, mm, ss, 0, 0, 0))
        except (ValueError, OverflowError):
            continue
        cur = (off, abbr)
        if prev is None:
            ivals.append((_NEG, off, abbr))
            prev = cur
        if cur != prev:
            ivals.append((ep, off, abbr))
            prev = cur
    ivals = [(a, o, ab) for (a, o, ab) in ivals if a < _CAP]
    if not ivals:
        # A fixed-offset zone (no transitions): take the constant offset and the
        # abbreviation CPython reports for it at the epoch.
        z = zoneinfo.ZoneInfo(zone)
        dt0 = datetime.fromtimestamp(0, tz=z)
        off = int(z.utcoffset(dt0).total_seconds())
        ivals = [(_NEG, off, z.tzname(dt0) or "")]
    return ivals


def ref_offset(zone, ep):
    """CPython zoneinfo's offset (seconds) at UTC instant `ep` — the oracle."""
    z = zoneinfo.ZoneInfo(zone)
    return int(z.utcoffset(datetime.fromtimestamp(ep, tz=z)).total_seconds())


def ref_abbr(zone, ep):
    """CPython zoneinfo's abbreviation at UTC instant `ep` — the abbr oracle."""
    z = zoneinfo.ZoneInfo(zone)
    return z.tzname(datetime.fromtimestamp(ep, tz=z)) or ""


def main(argv):
    validate = "--validate" in argv[1:]
    zones = sorted(zoneinfo.available_timezones())

    tables = {}  # intervals-tuple -> table id (dedups linked/aliased zones)
    order = []  # table id -> intervals
    zone_tbl = {}  # zone name -> table id
    total = 0
    mismatches = 0

    for z in zones:
        iv = intervals(z)
        if validate:
            for i, (a, o, ab) in enumerate(iv):
                nxt = iv[i + 1][0] if i + 1 < len(iv) else a + 1
                mid = (nxt - 1) if a == _NEG else (a + nxt) // 2
                if mid < _YEAR1:
                    mid = nxt - 1
                try:
                    if ref_offset(z, mid) != o:
                        mismatches += 1
                        print(f"MISMATCH {z} @ {mid}: {o}", file=sys.stderr)
                    if ref_abbr(z, mid) != ab:
                        mismatches += 1
                        print(
                            f"ABBR-MISMATCH {z} @ {mid}: {ab!r} "
                            f"!= {ref_abbr(z, mid)!r}",
                            file=sys.stderr,
                        )
                except (ValueError, OverflowError, KeyError, OSError):
                    # OSError (EOVERFLOW) is raised by the platform C library on
                    # macOS for a `fromtimestamp` instant outside its `time_t`
                    # range; such an extreme (pre-standard / far-future) instant
                    # is simply skipped, as the other out-of-range errors are.
                    pass
        key = tuple(iv)
        if key not in tables:
            tables[key] = len(order)
            order.append(iv)
            total += len(iv)
        zone_tbl[z] = tables[key]

    try:
        version = (
            subprocess.run(
                ["zdump", "--version"], capture_output=True, text=True
            ).stdout.strip()
            or "unknown"
        )
    except OSError:
        version = "unknown"

    # Intern the abbreviations into a small, order-stable global pool; each
    # transition stores an int index into it (#1345). The index fills the 4
    # bytes the `zap_tz_trans` struct already padded for 8-byte alignment, so
    # the transition array does not grow — only the pool (a few hundred short
    # strings) is added.
    abbr_idx = {}
    abbr_order = []
    for iv in order:
        for (_a, _o, ab) in iv:
            if ab not in abbr_idx:
                abbr_idx[ab] = len(abbr_order)
                abbr_order.append(ab)

    emit = sys.stdout.write
    emit(
        "/* GENERATED by crates/codegen/tools/gen_tzdata.py -- DO NOT EDIT.\n"
        " * Bundled IANA timezone transition tables (#1045, D-0225; abbreviations"
        " #1345).\n"
        f" * {len(zones)} zones, {len(order)} unique tables, {total} transitions,"
        f" {len(abbr_order)} abbreviations.\n"
        f" * tz tooling: {version}\n"
        " * Regenerate: python3 crates/codegen/tools/gen_tzdata.py \\\n"
        " *     > crates/codegen/src/runtime/tzdata_generated.inc.c\n"
        " * A zone's offset at a UTC instant is the `off` of the last interval\n"
        " * whose `at` is <= the instant (binary search); interval 0 starts at\n"
        " * INT64_MIN, so every instant resolves. Offsets are seconds east of\n"
        " * UTC; future transitions are expanded to year 2100. Each interval's\n"
        " * `abbr` indexes ZAP_TZ_ABBRS[] (the IANA zone abbreviation at that\n"
        " * instant, e.g. EST/EDT, or a numeric form like -04, or LMT). */\n"
    )
    emit(
        "typedef struct { int64_t at; int32_t off; int32_t abbr; }"
        " zap_tz_trans;\n"
    )
    emit(
        "typedef struct { const char *name; const zap_tz_trans *trans;"
        " int32_t n; } zap_tz_zone;\n"
    )
    pool = ",".join('"%s"' % a for a in abbr_order)
    emit("static const char *const ZAP_TZ_ABBRS[]={%s};\n" % pool)
    emit("static const int ZAP_TZ_NABBRS=%d;\n" % len(abbr_order))
    for i, iv in enumerate(order):
        body = ",".join(
            ("{INT64_MIN,%d,%d}" % (o, abbr_idx[ab]))
            if a == _NEG
            else ("{%dLL,%d,%d}" % (a, o, abbr_idx[ab]))
            for a, o, ab in iv
        )
        emit("static const zap_tz_trans ZAP_TZ_T%d[]={%s};\n" % (i, body))
    emit("static const zap_tz_zone ZAP_TZ_ZONES[]={\n")
    for z in zones:
        t = zone_tbl[z]
        emit('{"%s",ZAP_TZ_T%d,%d},\n' % (z, t, len(order[t])))
    emit("};\n")
    emit("static const int ZAP_TZ_NZONES=%d;\n" % len(zones))

    if validate:
        print(
            f"validated {len(zones)} zones, {total} transitions,"
            f" mismatches={mismatches}",
            file=sys.stderr,
        )
        return 1 if mismatches else 0
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
