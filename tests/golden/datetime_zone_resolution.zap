# Golden fixture: named-zone retention + civil-time resolution (#1149).
#
# Deterministic and host-tz-independent: every zone is the BUNDLED IANA data, so
# the output is fixed against the toolchain's tables, not the host. Covers:
#   - zone retention: astimezone then replace() across the New York spring
#     boundary (local noon Mar 9 -05:00 -> Mar 10 -04:00);
#   - gap (imaginary) classification + resolve_imaginary (02:30 -> 03:30);
#   - overlap (ambiguous) classification + fold selection (the two 01:30
#     instants one hour apart);
#   - pure-Python wall-clock add(): advancing the naive wall clock and
#     re-resolving the offset — a day's worth of seconds changes the offset, and
#     an add that lands in a gap / overlap resolves via the civil rules;
#   - a non-hour DST transition, Australia/Lord_Howe (30-minute gap);
#   - a fixed-offset zone (never a gap or an overlap).
# Identical under both collectors.
import datetime
import time


def main():
    auto ny = datetime.zone("America/New_York")

    # Retention: a named-zone value re-applies the zone's rule under replace().
    auto instant = datetime.rfc3339_parse("2024-03-09T17:00:00Z")
    auto local = datetime.astimezone(instant, ny)
    print(datetime.isoformat(local))  # 2024-03-09T12:00:00-05:00
    print(datetime.isoformat(datetime.replace(local, day=10)))  # ...-04:00

    # Spring-forward GAP: 2024-03-10 02:30 never occurs.
    auto gap = datetime.resolve(ny, 2024, 3, 10, 2, 30, 0)
    print(datetime.isoformat(gap))  # 2024-03-10T02:30:00-05:00
    print(datetime.is_imaginary(gap))  # True
    auto shifted = datetime.resolve_imaginary(gap)
    print(datetime.isoformat(shifted))  # 2024-03-10T03:30:00-04:00
    print(datetime.is_imaginary(shifted))  # False

    # Fall-back OVERLAP: 2024-11-03 01:30 occurs twice, one hour apart.
    auto early = datetime.resolve(ny, 2024, 11, 3, 1, 30, 0)
    auto late = datetime.resolve(ny, 2024, 11, 3, 1, 30, 0, fold=1)
    print(datetime.isoformat(early))  # 2024-11-03T01:30:00-04:00
    print(datetime.isoformat(late))  # 2024-11-03T01:30:00-05:00
    print(datetime.is_ambiguous(early))  # True
    print(datetime.to_timestamp(late) - datetime.to_timestamp(early))  # 3600

    # Wall-clock add(): a day's worth of seconds (86400) advances the naive wall
    # clock one calendar day and re-resolves the offset (-05:00 -> -04:00). In
    # Python timedelta(days=1) == timedelta(seconds=86400); both are naive-field
    # additions, so the flat Duration is sufficient.
    auto noon9 = datetime.resolve(ny, 2024, 3, 9, 12, 0, 0)
    print(datetime.isoformat(datetime.add(noon9, time.seconds(86400))))  # Mar10 -04:00

    # add() landing in the spring-forward GAP: 01:30 + 1h of wall clock = 02:30,
    # which is imaginary; the offset re-resolves to the pre-transition -05:00.
    auto pre_gap = datetime.resolve(ny, 2024, 3, 10, 1, 30, 0)
    auto into_gap = datetime.add(pre_gap, time.seconds(3600))
    print(datetime.isoformat(into_gap))  # 2024-03-10T02:30:00-05:00
    print(datetime.is_imaginary(into_gap))  # True

    # add() landing in the fall-back OVERLAP: 00:30 + 1h = 01:30, ambiguous; the
    # fold-0 (earlier) offset -04:00 is selected.
    auto pre_overlap = datetime.resolve(ny, 2024, 11, 3, 0, 30, 0)
    auto into_overlap = datetime.add(pre_overlap, time.seconds(3600))
    print(datetime.isoformat(into_overlap))  # 2024-11-03T01:30:00-04:00
    print(datetime.is_ambiguous(into_overlap))  # True

    # Non-hour transition: Australia/Lord_Howe, a 30-minute spring-forward gap.
    auto lh = datetime.zone("Australia/Lord_Howe")
    auto lhgap = datetime.resolve(lh, 2024, 10, 6, 2, 15, 0)
    print(datetime.is_imaginary(lhgap))  # True
    print(datetime.isoformat(datetime.resolve_imaginary(lhgap)))  # 02:45+11:00
    # add() across the same 30-minute gap: 01:45 + 30min = 02:15 (imaginary).
    auto lh_pre = datetime.resolve(lh, 2024, 10, 6, 1, 45, 0)
    auto lh_into = datetime.add(lh_pre, time.seconds(1800))
    print(datetime.isoformat(lh_into))  # 2024-10-06T02:15:00+10:30
    print(datetime.is_imaginary(lh_into))  # True

    # Fixed-offset zone: never a gap or an overlap.
    auto utcm5 = datetime.fixed_offset(-300)
    auto fx = datetime.resolve(utcm5, 2024, 3, 10, 2, 30, 0)
    print(datetime.isoformat(fx))  # 2024-03-10T02:30:00-05:00
    print(datetime.is_imaginary(fx))  # False
    print(datetime.is_ambiguous(fx))  # False
