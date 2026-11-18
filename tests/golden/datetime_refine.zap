# Golden fixture: the #974 deferred refinements (#1055, D-0228). Deterministic
# (fixed instants/offsets, no now()/since()): Go-style Duration formatting, the
# Date/Time comparison operators + weekday/day_of_year helpers, and sub-minute
# UTC offsets rendered to the second. Identical under both collectors.
import datetime
import time


def main():
    # time.format — Go time.Duration style.
    print(time.format(time.nanos(42)))  # 42ns
    print(time.format(time.micros(7)))  # 7us
    print(time.format(time.micros(7) + time.nanos(500)))  # 7.5us
    print(time.format(time.millis(250)))  # 250ms
    print(time.format(time.seconds(1) + time.millis(500)))  # 1.5s
    print(time.format(time.seconds(90) + time.millis(500)))  # 1m30.5s
    print(time.format(time.seconds(3661)))  # 1h1m1s
    print(time.format(time.seconds(0)))  # 0s
    # A negative Duration (diff where a < b).
    auto a = datetime.from_timestamp(0)
    auto b = datetime.from_timestamp(20)
    print(time.format(datetime.diff(a, b)))  # -20s

    # Sub-minute UTC offset rendered to the second (Asia/Kolkata MMT in 1900 =
    # +05:21:10) vs a whole-minute fixed offset (+05:30).
    auto kol = datetime.zone("Asia/Kolkata")
    auto old = datetime.astimezone(datetime.from_timestamp(-2208988800), kol)
    print(datetime.isoformat(old))  # 1900-01-01T05:21:10+05:21:10
    print(datetime.strftime(old, "%z"))  # +052110
    print(datetime.strftime(old, "%Z"))  # UTC+05:21:10
    auto ist = datetime.astimezone(
        datetime.from_timestamp(1609459200),
        datetime.fixed_offset(330),
    )
    print(datetime.isoformat(ist))  # 2021-01-01T05:30:00+05:30
    print(datetime.strftime(ist, "%z"))  # +0530

    # Date / Time comparisons (derived chronological order).
    auto d1 = datetime.date(2020, 1, 1)
    auto d2 = datetime.date(2021, 6, 15)
    print(d1 < d2)  # True
    print(d1 == d1)  # True
    print(d2 < d1)  # False
    auto t1 = datetime.time(9, 0, 0)
    auto t2 = datetime.time(17, 30, 0)
    print(t1 < t2)  # True

    # Date field helpers (2021-01-01 is a Friday; 2020 is a leap year).
    auto fri = datetime.date(2021, 1, 1)
    print(datetime.date_weekday(fri))  # 4
    print(datetime.date_isoweekday(fri))  # 5
    print(datetime.date_day_of_year(fri))  # 1
    print(datetime.date_day_of_year(datetime.date(2020, 12, 31)))  # 366
