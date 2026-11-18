# Golden fixture: datetime.replace() + the separate Date / Time types (#1046,
# D-0226). Deterministic — fixed UTC instants and fixed offsets only (no now() /
# date_today(), which are host-local). replace() is a civil-field substitution
# that preserves the zone (offset / is_aware); Date/Time are standalone civil
# values rendered by the overloaded isoformat; combine() rebuilds a naive
# DateTime. Identical under both collectors.
import datetime


def main():
    # 2021-06-01T00:00:00Z (naive).
    auto dt = datetime.from_timestamp(1622505600)

    # replace(): override some civil fields, keep the rest; naive stays naive.
    auto r = datetime.replace(dt, year=1999, day=15)
    print(datetime.isoformat(r))  # 1999-06-15T00:00:00
    print(r.year)  # 1999
    print(r.month)  # 6 (kept)
    print(r.day)  # 15

    # replace() preserves the zone (offset + is_aware).
    auto ist = datetime.fixed_offset(330)
    auto aware = datetime.astimezone(dt, ist)
    auto r2 = datetime.replace(aware, hour=9, minute=0, second=0)
    print(datetime.isoformat(r2))  # 2021-06-01T09:00:00+05:30
    print(r2.offset)  # 19800

    # Date / Time constructors + the overloaded isoformat.
    auto d = datetime.date(2020, 2, 29)
    auto t = datetime.time(13, 5, 9)
    print(datetime.isoformat(d))  # 2020-02-29
    print(datetime.isoformat(t))  # 13:05:09
    print(d.year)  # 2020
    print(t.minute)  # 5

    # Extraction from a DateTime.
    auto d2 = datetime.to_date(dt)
    auto t2 = datetime.to_time(dt)
    print(datetime.isoformat(d2))  # 2021-06-01
    print(datetime.isoformat(t2))  # 00:00:00

    # Combine a Date + Time into a naive DateTime.
    auto combined = datetime.combine(d, t)
    print(datetime.isoformat(combined))  # 2020-02-29T13:05:09
    print(datetime.to_timestamp(combined))  # 1582981509
