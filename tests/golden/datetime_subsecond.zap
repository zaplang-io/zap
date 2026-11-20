# Golden fixture: subsecond precision on the core `DateTime` (#1148, ADR
# decisions/stdlib/1148-subsecond-datetime.md). Core `DateTime` carries an
# `int64 nanosecond` (nanoseconds-of-second); `add`/`diff` carry the sub-second
# remainder instead of truncating the `Duration` (#1047) to whole seconds;
# `replace` gains `microsecond=` / `nanosecond=`; and the ISO/RFC-3339 renderer
# + parser round-trip a fractional component. Driven from fixed, timezone-
# independent values (parse/from_timestamp are UTC) so the output is
# deterministic. Identical under both collectors.
import datetime
import time


def main():
    # --- the issue reproduction: +1us is preserved ---
    auto start = datetime.parse("2024-01-01T00:00:00")
    auto nxt = datetime.add(start, time.micros(1))
    print(datetime.equal(start, nxt))  # False
    print(datetime.isoformat(nxt))  # 2024-01-01T00:00:00.000001
    print(datetime.before(start, nxt))  # True
    print(datetime.after(nxt, start))  # True
    # same second, different fraction -> ordered (comparisons observe it).
    print(datetime.before(nxt, start))  # False

    # --- microsecond-scale diff observes the fraction ---
    print(time.to_micros(datetime.diff(nxt, start)))  # 1.0
    print(time.to_micros(datetime.diff(start, nxt)))  # -1.0

    # --- carry across a second boundary rolls the day/month/year ---
    auto edge = datetime.replace(
        datetime.parse("2024-01-01T23:59:59"),
        microsecond=999999,
    )
    print(datetime.isoformat(datetime.add(edge, time.micros(1))))  # 2024-01-02T00:00:00
    auto yend = datetime.replace(
        datetime.parse("2024-12-31T23:59:59"),
        microsecond=999999,
    )
    print(datetime.isoformat(datetime.add(yend, time.micros(1))))  # 2025-01-01T00:00:00

    # --- borrow across a day boundary (negative delta) ---
    auto day0 = datetime.parse("2024-03-02T00:00:00")
    print(datetime.isoformat(datetime.add(day0, time.micros(-1))))  # 2024-03-01T23:59:59.999999

    # --- replace: microsecond= (x1000) and nanosecond= (exact); 6 vs 9 digits ---
    auto base = datetime.parse("2024-06-15T12:30:45")
    print(datetime.isoformat(datetime.replace(base, microsecond=250000)))  # .250000
    print(datetime.isoformat(datetime.replace(base, nanosecond=123456789)))  # .123456789

    # nanosecond headroom + replace preserves the fraction through a civil edit.
    auto frac = datetime.replace(base, nanosecond=1)
    print(datetime.isoformat(datetime.replace(frac, day=20)))  # 2024-06-20T12:30:45.000000001
    # replacing a civil field with no fractional keyword keeps the fraction.
    print(datetime.isoformat(datetime.replace(nxt, hour=6)))  # 2024-01-01T06:00:00.000001

    # --- fixed-offset round-trip with an explicit offset ---
    auto aware = datetime.rfc3339_parse("2024-01-01T00:00:00.000001+05:30")
    print(datetime.isoformat(aware))  # 2024-01-01T00:00:00.000001+05:30

    # --- pre-epoch value keeps the fraction ---
    auto pre = datetime.replace(
        datetime.parse("1969-07-20T20:17:40"),
        microsecond=500000,
    )
    print(datetime.isoformat(pre))  # 1969-07-20T20:17:40.500000

    # --- parser accepts a variable-length fraction (1..9 digits) ---
    print(datetime.isoformat(datetime.parse("2024-01-01T00:00:00.1")))  # .100000
    print(datetime.isoformat(datetime.parse("2024-01-01T00:00:00.123")))  # .123000
    print(datetime.isoformat(datetime.parse("2024-01-01T00:00:00.123456789")))  # .123456789
    # digits past the 9th are truncated.
    print(datetime.isoformat(datetime.parse("2024-01-01T00:00:00.1234567899")))  # .123456789

    # --- whole-second values are unchanged (no fraction emitted) ---
    print(datetime.isoformat(datetime.parse("2024-01-01T00:00:00")))  # 2024-01-01T00:00:00
    print(datetime.format(datetime.parse("2024-01-01T00:00:00")))  # 2024-01-01T00:00:00
    print(datetime.equal(start, datetime.parse("2024-01-01T00:00:00")))  # True
