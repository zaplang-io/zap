# Golden fixture: the `datetime` timezone database (#1045, D-0225).
#
# Deterministic and host-tz-independent: fixed UTC instants + fixed offsets and
# the BUNDLED `Asia/Kolkata` zone (the data ships in the toolchain, so named-zone
# output does not depend on the host). Covers `astimezone` across a fixed offset
# and a named zone (a modern instant -> +05:30, NOT the +05:53 LMT footgun; an
# early-20th-century instant -> the historical +05:21:10, preserved), aware
# `isoformat`/`rfc3339` with +/-HH:MM and +00:00, `rfc3339_parse` round-trips,
# and `%z`/`%Z` on an aware value (empty on a naive one). Identical under both
# collectors.
import datetime


def main():
    # 2021-06-01T00:00:00Z.
    auto dt = datetime.from_timestamp(1622505600)
    print(datetime.isoformat(dt))  # 2021-06-01T00:00:00  (naive: no suffix)

    # Fixed offset +05:30.
    auto ist = datetime.fixed_offset(330)
    auto a = datetime.astimezone(dt, ist)
    print(datetime.isoformat(a))  # 2021-06-01T05:30:00+05:30
    print(datetime.rfc3339(a))  # same (alias)
    print(a.hour)  # 5
    print(a.minute)  # 30
    print(a.offset)  # 19800
    print(datetime.to_timestamp(a))  # 1622505600 (instant preserved)

    # Named zone at a modern instant -> +05:30 (NOT the +05:53 LMT).
    auto kol = datetime.zone("Asia/Kolkata")
    auto k = datetime.astimezone(dt, kol)
    print(datetime.isoformat(k))  # 2021-06-01T05:30:00+05:30
    print(k.offset)  # 19800

    # Same zone at 1900-01-01T00:00:00Z -> historical +05:21:10, preserved.
    auto old = datetime.from_timestamp(-2208988800)
    auto ko = datetime.astimezone(old, kol)
    print(ko.offset)  # 19270 (5:21:10 MMT, not the modern 5:30)
    print(ko.hour)  # 5
    print(ko.minute)  # 21
    print(ko.second)  # 10

    # Negative fixed offset -05:00.
    auto est = datetime.fixed_offset(-300)
    auto e = datetime.astimezone(dt, est)
    print(datetime.isoformat(e))  # 2021-05-31T19:00:00-05:00
    print(datetime.to_timestamp(e))  # 1622505600

    # strftime %z / %Z on an aware value.
    print(datetime.strftime(a, "%H:%M %z %Z"))  # 05:30 +0530 UTC+05:30

    # A naive value still renders %z / %Z empty.
    print(datetime.strftime(dt, "|%z|%Z|"))  # |||

    # UTC-aware zero offset prints +00:00.
    auto utc0 = datetime.fixed_offset(0)
    auto z0 = datetime.astimezone(dt, utc0)
    print(datetime.isoformat(z0))  # 2021-06-01T00:00:00+00:00

    # rfc3339_parse round-trips an explicit offset.
    auto p = datetime.rfc3339_parse("2021-06-01T05:30:00+05:30")
    print(datetime.to_timestamp(p))  # 1622505600
    print(p.offset)  # 19800
    print(datetime.isoformat(p))  # 2021-06-01T05:30:00+05:30

    # rfc3339_parse with Z -> aware, zero offset.
    auto pz = datetime.rfc3339_parse("2021-06-01T00:00:00Z")
    print(pz.offset)  # 0
    print(datetime.isoformat(pz))  # 2021-06-01T00:00:00+00:00
