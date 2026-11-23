# Golden fixture: datetime.zone_abbreviation(dt) -> Optional[str] (#1345).
#
# Deterministic and host-tz-independent: every zone is the BUNDLED IANA data, so
# the abbreviations are fixed against the toolchain's tables, not the host.
# Covers:
#   - the America/New_York fall-back overlap: the 01:30 instant resolved with
#     fold=0 reports EDT, with fold=1 reports EST;
#   - the SAME abbreviations when the instants are obtained by converting FROM
#     UTC (astimezone of a UTC instant) rather than resolving wall time;
#   - standard winter/summer (EST/EDT) and a European zone (CET/CEST);
#   - a historical pre-standard instant (LMT) and a numeric abbreviation
#     (Asia/Yangon's +0630), faithful to tzdata / CPython zoneinfo.tzname();
#   - a naive value and a fixed-offset value both yield None (an offset alone
#     does not establish an abbreviation);
#   - strftime's %Z is unchanged: still the numeric offset, never the name.
# Identical under both collectors.
import datetime


# Render an Optional[str] so the output is a fixed string either way.
def show(Optional[str] a) -> str:
    match a:
        case Optional.Some(v):
            return v
        case Optional.None:
            return "<none>"


def main():
    auto ny = datetime.zone("America/New_York")

    # Fall-back OVERLAP: 2024-11-03 01:30 occurs twice. fold=0 is the earlier
    # (EDT, -04:00) instant; fold=1 the later (EST, -05:00) instant.
    auto early = datetime.resolve(ny, 2024, 11, 3, 1, 30, 0)
    auto late = datetime.resolve(ny, 2024, 11, 3, 1, 30, 0, fold=1)
    print(show(datetime.zone_abbreviation(early)))  # EDT
    print(show(datetime.zone_abbreviation(late)))  # EST

    # The SAME two instants, built by converting FROM UTC instead of resolving a
    # wall clock: 05:30Z is 01:30 EDT, 06:30Z is 01:30 EST.
    auto u_edt = datetime.astimezone(datetime.rfc3339_parse("2024-11-03T05:30:00Z"), ny)
    auto u_est = datetime.astimezone(datetime.rfc3339_parse("2024-11-03T06:30:00Z"), ny)
    print(show(datetime.zone_abbreviation(u_edt)))  # EDT
    print(show(datetime.zone_abbreviation(u_est)))  # EST

    # Standard winter / summer.
    auto winter = datetime.astimezone(
        datetime.rfc3339_parse("2024-01-15T12:00:00Z"),
        ny,
    )
    auto summer = datetime.astimezone(
        datetime.rfc3339_parse("2024-07-15T12:00:00Z"),
        ny,
    )
    print(show(datetime.zone_abbreviation(winter)))  # EST
    print(show(datetime.zone_abbreviation(summer)))  # EDT

    # A European zone: CET in winter, CEST in summer.
    auto paris = datetime.zone("Europe/Paris")
    auto p_cet = datetime.astimezone(
        datetime.rfc3339_parse("2024-01-15T12:00:00Z"),
        paris,
    )
    auto p_cest = datetime.astimezone(
        datetime.rfc3339_parse("2024-07-15T12:00:00Z"),
        paris,
    )
    print(show(datetime.zone_abbreviation(p_cet)))  # CET
    print(show(datetime.zone_abbreviation(p_cest)))  # CEST

    # Historical pre-standard-time instant for New York: Local Mean Time.
    auto lmt = datetime.astimezone(datetime.rfc3339_parse("1850-01-01T12:00:00Z"), ny)
    print(show(datetime.zone_abbreviation(lmt)))  # LMT

    # A numeric abbreviation, faithful to tzdata (CPython zoneinfo.tzname()).
    auto yangon = datetime.zone("Asia/Yangon")
    auto y = datetime.astimezone(datetime.rfc3339_parse("2024-01-15T12:00:00Z"), yangon)
    print(show(datetime.zone_abbreviation(y)))  # +0630

    # A naive value carries no named zone.
    auto naive = datetime.from_timestamp(0)
    print(show(datetime.zone_abbreviation(naive)))  # <none>

    # A fixed-offset value carries no named zone either.
    auto fixed = datetime.astimezone(
        datetime.from_timestamp(0),
        datetime.fixed_offset(330),
    )
    print(show(datetime.zone_abbreviation(fixed)))  # <none>

    # %Z is unchanged: still the numeric offset, not the abbreviation.
    print(datetime.strftime(summer, "%Z"))  # UTC-04:00
    print(datetime.strftime(winter, "%Z"))  # UTC-05:00
