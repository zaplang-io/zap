# Golden fixture: datetime arithmetic / comparisons / field helpers (ticket
# #974, D-0221; durations are the reconciled `Duration` type since #1047,
# D-0227). Uses fixed UTC timestamps (not now()) so the output is deterministic
# and host-timezone independent (from_timestamp is UTC).
import datetime
import time


def main():
    # 2021-01-01 00:00:00 UTC (a Friday).
    auto dt = datetime.from_timestamp(1609459200)
    print(datetime.format(dt))  # 2021-01-01T00:00:00
    print(datetime.weekday(dt))  # 4 (Friday, Mon=0)
    print(datetime.isoweekday(dt))  # 5
    print(datetime.day_of_year(dt))  # 1

    # add one day (a Duration).
    auto dt2 = datetime.add(dt, time.seconds(86400))
    print(datetime.format(dt2))  # 2021-01-02T00:00:00
    print(datetime.weekday(dt2))  # 5 (Saturday)
    print(datetime.day_of_year(dt2))  # 2

    # diff -> Duration; render as whole seconds.
    print(time.to_seconds(datetime.diff(dt2, dt)))  # 86400.0

    print(datetime.before(dt, dt2))  # True
    print(datetime.after(dt, dt2))  # False
    print(datetime.equal(dt, dt))  # True

    # 2020-12-31 00:00:00 UTC — a leap year, so day-of-year 366 (a Thursday).
    auto ly = datetime.from_timestamp(1609372800)
    print(datetime.day_of_year(ly))  # 366
    print(datetime.weekday(ly))  # 3 (Thursday)
