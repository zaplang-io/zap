# Golden fixture: the reconciled `Duration` type (#1047, D-0227). Deterministic
# (no clocks): construction, the D-0047 dunder operators (+/-/*/comparisons),
# the unit converters, and the datetime bridge (datetime.add takes a Duration,
# datetime.diff returns one). Identical under both collectors.
import datetime
import time


def main():
    auto s = time.seconds(2)
    auto ms = time.millis(500)
    print(s.nanos)  # 2000000000
    print(ms.nanos)  # 500000000

    # Operators (Duration op Duration; scalar scaling; comparisons).
    auto sum = s + ms
    print(sum.nanos)  # 2500000000
    auto delta = s - ms
    print(delta.nanos)  # 1500000000
    auto scaled = ms * 3
    print(scaled.nanos)  # 1500000000
    print(s < ms)  # False
    print(ms < s)  # True
    print(s == time.seconds(2))  # True

    # Converters (Duration -> float unit).
    print(time.to_seconds(s))  # 2.0
    print(time.to_millis(ms))  # 500.0
    print(time.to_micros(time.micros(7)))  # 7.0

    # datetime bridge: add takes a Duration, diff returns one.
    auto dt = datetime.from_timestamp(1609459200)  # 2021-01-01T00:00:00
    auto dt2 = datetime.add(dt, time.seconds(90061))  # +1d 01:01:01
    print(datetime.format(dt2))  # 2021-01-02T01:01:01
    auto d = datetime.diff(dt2, dt)
    print(d.nanos)  # 90061000000000
    print(time.to_seconds(d))  # 90061.0

    # A sub-second Duration truncates in whole-second datetime math.
    auto dt3 = datetime.add(dt, time.millis(1500))  # 1.5s -> 1s
    print(datetime.to_timestamp(dt3))  # 1609459201
