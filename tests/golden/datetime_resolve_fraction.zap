# Golden fixture: resolve a named-zone wall clock with BOTH a DST fold and a
# fractional second in one call (#1361, ADR
# decisions/stdlib/1361-resolve-fold-fraction.md). `datetime.resolve` gains a
# keyword-only `nanosecond=` selector alongside the existing `fold`, so a chosen
# overlap instant and a sub-second fraction can be constructed together — the
# combination the datetime-utils parser needs and that `replace` (fold-0 only)
# could not express. Every zone is the BUNDLED IANA data, so the output is fixed
# against the toolchain's tables, not the host. Identical under both collectors.
import datetime


# Print an Optional[str] abbreviation (EST/EDT/…) or a placeholder — the device
# the datetime-utils parser uses to tell the two overlap occurrences apart.
def show(Optional[str] a) -> str:
    match a:
        case Optional.Some(v):
            return v
        case Optional.None:
            return "<none>"


def main():
    auto ny = datetime.zone("America/New_York")

    # === Headline: a fold AND a nine-digit fraction together ===
    # Fall-back OVERLAP 2024-11-03 01:30 occurs twice. fold=1 is the later (EST,
    # -18000) instant; fold=0 the earlier (EDT, -14400). BOTH retain all nine
    # fractional digits.
    auto late = datetime.resolve(
        ny,
        2024,
        11,
        3,
        1,
        30,
        0,
        fold=1,
        nanosecond=123456789,
    )
    auto early = datetime.resolve(
        ny,
        2024,
        11,
        3,
        1,
        30,
        0,
        fold=0,
        nanosecond=123456789,
    )
    print(datetime.isoformat(late))  # 2024-11-03T01:30:00.123456789-05:00
    print(late.offset)  # -18000
    print(late.nanosecond)  # 123456789
    print(show(datetime.zone_abbreviation(late)))  # EST
    print(datetime.isoformat(early))  # 2024-11-03T01:30:00.123456789-04:00
    print(early.offset)  # -14400
    print(early.nanosecond)  # 123456789
    print(show(datetime.zone_abbreviation(early)))  # EDT

    # === Unique (non-ambiguous) wall clock with a fraction ===
    # Mid-summer: a single EDT occurrence. A whole number of microseconds renders
    # as 6 digits (Python isoformat); the field keeps full nanosecond precision.
    auto uniq = datetime.resolve(ny, 2024, 7, 15, 12, 0, 0, nanosecond=250000000)
    print(datetime.isoformat(uniq))  # 2024-07-15T12:00:00.250000-04:00
    print(uniq.offset)  # -14400
    print(uniq.nanosecond)  # 250000000

    # === Fixed-offset zone (never a gap/overlap) with a fraction ===
    auto utcm5 = datetime.fixed_offset(-300)
    auto fx = datetime.resolve(utcm5, 2024, 3, 10, 2, 30, 0, nanosecond=1)
    print(datetime.isoformat(fx))  # 2024-03-10T02:30:00.000000001-05:00
    print(fx.offset)  # -18000
    print(fx.nanosecond)  # 1

    # === Fractional-range boundaries ===
    # Low boundary: nanosecond=0 is whole seconds — the fraction is omitted,
    # identical to the historical behaviour and to omitting the keyword.
    auto lo = datetime.resolve(ny, 2024, 7, 15, 12, 0, 0, nanosecond=0)
    print(datetime.isoformat(lo))  # 2024-07-15T12:00:00-04:00
    print(lo.nanosecond)  # 0
    # High boundary: the maximum in-range fraction, nine 9s.
    auto hi = datetime.resolve(ny, 2024, 7, 15, 12, 0, 0, nanosecond=999999999)
    print(datetime.isoformat(hi))  # 2024-07-15T12:00:00.999999999-04:00
    print(hi.nanosecond)  # 999999999

    # === Default (omitted nanosecond) stays whole-second, unchanged ===
    auto whole = datetime.resolve(ny, 2024, 7, 15, 12, 0, 0)
    print(datetime.isoformat(whole))  # 2024-07-15T12:00:00-04:00
    print(whole.nanosecond)  # 0

    # === Lenient storage: an out-of-range fraction is kept verbatim, like
    # replace() and the lenient civil fields (no range validation). A value
    # >= 1e9 is stored in the field; isoformat omits an out-of-range fraction. ===
    auto oor = datetime.resolve(ny, 2024, 7, 15, 12, 0, 0, nanosecond=1000000000)
    print(oor.nanosecond)  # 1000000000
    print(datetime.isoformat(oor))  # 2024-07-15T12:00:00-04:00

    # === Gap (imaginary) wall clock with a fraction: resolve_imaginary shifts
    # the whole-second wall clock forward and the fraction rides through. ===
    auto gap = datetime.resolve(ny, 2024, 3, 10, 2, 30, 0, nanosecond=123456789)
    print(datetime.is_imaginary(gap))  # True
    auto shifted = datetime.resolve_imaginary(gap)
    print(datetime.isoformat(shifted))  # 2024-03-10T03:30:00.123456789-04:00
    print(shifted.nanosecond)  # 123456789
