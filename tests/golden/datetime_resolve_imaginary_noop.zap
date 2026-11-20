# Golden fixture: `datetime.resolve_imaginary` is a strict no-op for a REAL wall
# clock (#1173, regression for #1149). Every zone is the BUNDLED IANA data, so
# the output is fixed against the toolchain's tables, not the host.
#
# The bug: on a fall-back OVERLAP both `fold=0` and `fold=1` are real times one
# offset-step apart, but `resolve_imaginary` snapped the later (`fold=1`) value
# to the EARLIER instant. The wall-clock fields looked unchanged, so this golden
# asserts the INSTANT (`to_timestamp`) and the OFFSET (`isoformat` suffix), not
# only the wall fields — the only way the regression is visible.
#
# Coverage:
#   - fall-back OVERLAP, BOTH folds, instant + offset preserved — New York
#     (1-hour step) AND Australia/Lord_Howe (30-minute step);
#   - unique, naive, and fixed-offset values pass through unchanged;
#   - genuinely IMAGINARY (gap) times still advance by the transition's offset
#     change onto the real post-gap wall clock (same instant) — a 1-hour gap
#     (New York) AND a half-hour gap (Lord_Howe).
# Identical under both collectors.
import datetime


def main():
    auto ny = datetime.zone("America/New_York")

    # Fall-back OVERLAP: 2024-11-03 01:30 occurs twice, one hour apart. Both
    # folds are REAL, so resolve_imaginary must return each UNCHANGED — same
    # instant, same offset.
    auto early = datetime.resolve(ny, 2024, 11, 3, 1, 30, 0)
    auto late = datetime.resolve(ny, 2024, 11, 3, 1, 30, 0, fold=1)
    print(datetime.is_ambiguous(early))  # True
    print(datetime.is_ambiguous(late))  # True
    print(datetime.is_imaginary(late))  # False
    # fold=0 (earlier, -04:00): no-op.
    auto ri_early = datetime.resolve_imaginary(early)
    print(datetime.equal(ri_early, early))  # True
    print(datetime.to_timestamp(ri_early) - datetime.to_timestamp(early))  # 0
    print(datetime.isoformat(ri_early))  # 2024-11-03T01:30:00-04:00
    # fold=1 (later, -05:00): THE regression — instant must NOT move earlier.
    auto ri_late = datetime.resolve_imaginary(late)
    print(datetime.equal(ri_late, late))  # True
    print(datetime.to_timestamp(ri_late) - datetime.to_timestamp(late))  # 0
    print(datetime.isoformat(ri_late))  # 2024-11-03T01:30:00-05:00

    # Non-hour fall-back OVERLAP: Australia/Lord_Howe, a 30-minute step
    # (2024-04-07 01:30..02:00 occurs twice: +11:00 then +10:30).
    auto lh = datetime.zone("Australia/Lord_Howe")
    auto lh_early = datetime.resolve(lh, 2024, 4, 7, 1, 45, 0)
    auto lh_late = datetime.resolve(lh, 2024, 4, 7, 1, 45, 0, fold=1)
    print(datetime.is_ambiguous(lh_late))  # True
    auto lh_ri_early = datetime.resolve_imaginary(lh_early)
    print(datetime.equal(lh_ri_early, lh_early))  # True
    print(datetime.to_timestamp(lh_ri_early) - datetime.to_timestamp(lh_early))  # 0
    print(datetime.isoformat(lh_ri_early))  # 2024-04-07T01:45:00+11:00
    auto lh_ri_late = datetime.resolve_imaginary(lh_late)
    print(datetime.equal(lh_ri_late, lh_late))  # True
    print(datetime.to_timestamp(lh_ri_late) - datetime.to_timestamp(lh_late))  # 0
    print(datetime.isoformat(lh_ri_late))  # 2024-04-07T01:45:00+10:30

    # Unique time (no transition nearby): unchanged.
    auto uniq = datetime.resolve(ny, 2024, 7, 1, 12, 0, 0)
    auto ri_uniq = datetime.resolve_imaginary(uniq)
    print(datetime.equal(ri_uniq, uniq))  # True
    print(datetime.to_timestamp(ri_uniq) - datetime.to_timestamp(uniq))  # 0
    print(datetime.isoformat(ri_uniq))  # 2024-07-01T12:00:00-04:00

    # Naive value (no zone): always unique, always a no-op.
    auto naive = datetime.from_timestamp(1700000000)
    print(datetime.is_imaginary(naive))  # False
    print(datetime.is_ambiguous(naive))  # False
    auto ri_naive = datetime.resolve_imaginary(naive)
    print(datetime.equal(ri_naive, naive))  # True
    print(datetime.to_timestamp(ri_naive) - datetime.to_timestamp(naive))  # 0
    print(datetime.isoformat(ri_naive))  # 2023-11-14T22:13:20

    # Fixed-offset zone: never a gap or an overlap, so a no-op.
    auto fx = datetime.resolve(datetime.fixed_offset(-300), 2024, 3, 10, 2, 30, 0)
    print(datetime.is_imaginary(fx))  # False
    auto ri_fx = datetime.resolve_imaginary(fx)
    print(datetime.equal(ri_fx, fx))  # True
    print(datetime.to_timestamp(ri_fx) - datetime.to_timestamp(fx))  # 0
    print(datetime.isoformat(ri_fx))  # 2024-03-10T02:30:00-05:00

    # Genuinely IMAGINARY (spring-forward GAP): still advance onto the real
    # post-gap wall clock. The same physical instant, re-labelled with the real
    # offset, so to_timestamp is unchanged while the wall clock and offset move.
    auto gap = datetime.resolve(ny, 2024, 3, 10, 2, 30, 0)
    print(datetime.is_imaginary(gap))  # True
    print(datetime.isoformat(gap))  # 2024-03-10T02:30:00-05:00
    auto ri_gap = datetime.resolve_imaginary(gap)
    print(datetime.is_imaginary(ri_gap))  # False
    print(datetime.isoformat(ri_gap))  # 2024-03-10T03:30:00-04:00
    print(datetime.to_timestamp(ri_gap) - datetime.to_timestamp(gap))  # 0

    # Half-hour spring-forward GAP: Lord_Howe 2024-10-06 02:15 advances +30min.
    auto lh_gap = datetime.resolve(lh, 2024, 10, 6, 2, 15, 0)
    print(datetime.is_imaginary(lh_gap))  # True
    print(datetime.isoformat(lh_gap))  # 2024-10-06T02:15:00+10:30
    auto lh_ri_gap = datetime.resolve_imaginary(lh_gap)
    print(datetime.is_imaginary(lh_ri_gap))  # False
    print(datetime.isoformat(lh_ri_gap))  # 2024-10-06T02:45:00+11:00
    print(datetime.to_timestamp(lh_ri_gap) - datetime.to_timestamp(lh_gap))  # 0
