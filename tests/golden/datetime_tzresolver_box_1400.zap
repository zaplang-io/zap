# Golden fixture: a `TimeZone` (a `value struct`) returned by value from a user
# zone resolver, wrapped in `Optional.Some`, carried up the call stack through a
# first-class-function callback and a tuple, then unwrapped in a *different*
# frame and fed to `datetime.resolve` — the datetime-utils parser shape that
# segfaulted in `zap_tz_zone_id` (zaplang-io/zap#1400).
#
# The crash was a lowering defect, not a datetime one: an inline-eligible value
# struct crosses a by-value return in a register / sret STACK spill, and storing
# that spill's pointer straight into the heap `Optional` payload left the field
# dangling once the resolver's frame returned. The fix boxes the payload onto
# the heap (the same boxing the interface-coercion and collection paths already
# do). This fixture drives the exact path end to end so a regression cannot
# silently reintroduce the dangling store. Every zone is the BUNDLED IANA data,
# so the output is fixed against the toolchain's tables, not the host; identical
# under both collectors.
import datetime


# A by-value factory: the returned `TimeZone` lives in the caller's sret/stack
# spill, exactly like datetime-utils' `tz.gettz`. (Not `datetime.zone` inline,
# which already lowers to a heap `StructNew`.)
def make_zone(str name) -> TimeZone:
    return datetime.zone(name)


# Print an Optional[str] abbreviation (EST/EDT/…) or a placeholder.
def show(Optional[str] a) -> str:
    match a:
        case Optional.Some(v):
            return v
        case Optional.None:
            return "<none>"


# A user resolver: `Some(make_zone(...))` auto-wraps a bare by-value `TimeZone`
# into `Optional[TimeZone]` (the #1262 auto-wrap path), or returns `None`.
def resolver(Optional[str] name, Optional[int64] offset) -> Optional[TimeZone]:
    if name.is_some() and name.unwrap() == "EST" and offset.is_none():
        return make_zone("America/New_York")
    return None


# The callback boundary: the resolver arrives as a first-class function value
# and its `Optional[TimeZone]` result leaves inside a tuple — so the `Some` is
# built in the resolver's frame, which returns before anyone unwraps it.
def choose_zone(
    Optional[fn(Optional[str], Optional[int64]) -> Optional[TimeZone]] tzresolver,
    Optional[str] name,
) -> (Optional[TimeZone], bool):
    if tzresolver.is_none():
        return (None, False)
    Optional[int64] seconds = None
    auto resolve = tzresolver.unwrap()
    return (resolve(name, seconds), True)


# The consumer frame (datetime-utils' `_build_result`): unwrap the zone that was
# produced in a now-returned frame and resolve the wall clock against it,
# inheriting the fractional second from `base` and disambiguating the fall-back
# overlap by abbreviation (fold 0 vs fold 1), just like the parser.
def build_result(Optional[TimeZone] zone, str zone_name, DateTime base) -> DateTime:
    auto nanos = base.nanosecond
    auto selected = zone.unwrap()
    auto aware = datetime.resolve(selected, 2024, 11, 3, 1, 30, 0, nanosecond=nanos)
    if zone_name != "" and datetime.is_ambiguous(aware):
        auto earlier_name = datetime.zone_abbreviation(aware)
        if earlier_name.is_none() or earlier_name.unwrap() != zone_name:
            auto later = datetime.resolve(
                selected,
                2024,
                11,
                3,
                1,
                30,
                0,
                fold=1,
                nanosecond=nanos,
            )
            auto later_name = datetime.zone_abbreviation(later)
            if later_name.is_some() and later_name.unwrap() == zone_name:
                return later
    return aware


def main():
    # A date default carrying an inherited sub-second fraction, exactly as the
    # parser builds it (`isoparse(date)` then `replace(nanosecond=...)`).
    auto midnight = datetime.resolve(
        datetime.zone("America/New_York"),
        2024,
        11,
        3,
        0,
        0,
        0,
    )
    auto base = datetime.replace(midnight, nanosecond=123456789)

    # === Some(TimeZone) through callback + tuple + a returned frame ===
    # The resolver matches "EST" with no offset, returns Some(America/New_York).
    auto zone, called = choose_zone(Optional.Some(resolver), Optional.Some("EST"))
    print(called)  # True
    print(zone.is_some())  # True

    # Fold disambiguation selects the LATER (EST, -18000) occurrence, inheriting
    # the 123456789 ns fraction — the acceptance result.
    auto stamp = build_result(zone, "EST", base)
    print(datetime.isoformat(stamp))  # 2024-11-03T01:30:00.123456789-05:00
    print(stamp.offset)  # -18000
    print(stamp.nanosecond)  # 123456789
    print(show(datetime.zone_abbreviation(stamp)))  # EST
    print(datetime.is_ambiguous(stamp))  # True

    # The .name of the unwrapped zone must survive the frame return (this is the
    # exact pointer that read null before the fix).
    auto selected = zone.unwrap()
    print(selected.name)  # America/New_York

    # === None return from the resolver (naive path) ===
    # A non-matching name resolves to None, so no zone is chosen.
    auto nzone, ncalled = choose_zone(Optional.Some(resolver), Optional.Some("ZZZ"))
    print(ncalled)  # True
    print(nzone.is_some())  # False

    # A missing resolver → (None, False), the ignoretz-style path.
    Optional[fn(Optional[str], Optional[int64]) -> Optional[TimeZone]] noresolver = None
    auto mzone, mcalled = choose_zone(noresolver, Optional.Some("EST"))
    print(mcalled)  # False
    print(mzone.is_some())  # False

    # === Direct Some(by-value result) then unwrap in main (fold 0 contrast) ===
    # Also pins the EARLIER (EDT, -14400) occurrence to prove the fraction rides
    # through both fold instants after boxing.
    Optional[TimeZone] direct = make_zone("America/New_York")
    auto tz = direct.unwrap()
    auto early = datetime.resolve(
        tz,
        2024,
        11,
        3,
        1,
        30,
        0,
        fold=0,
        nanosecond=base.nanosecond,
    )
    print(datetime.isoformat(early))  # 2024-11-03T01:30:00.123456789-04:00
    print(early.offset)  # -14400
    print(show(datetime.zone_abbreviation(early)))  # EDT
    print(early.nanosecond)  # 123456789
