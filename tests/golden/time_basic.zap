# Golden fixture: the `time` module (ticket #99, D-0056; durations are the
# reconciled `Duration` type since #1047, D-0227).
#
# A duration is now the prelude `Duration` value struct (one int64-ns field),
# with real `+`/`-`/`*`/comparison operators; `.nanos` reads the raw count. Unit
# construction/conversion is exact and deterministic. The clock checks are
# RELATIVE (monotonic elapsed across a sleep, wall clock past a fixed past
# instant), so the output is identical on every run and under both collectors.


def main():
    # Durations compose with real operators (Duration + Duration -> Duration).
    auto d = time.seconds(1) + time.millis(500)  # 1_500_000_000 ns
    print(d.nanos)  # 1500000000
    print(time.millis(250).nanos)  # 250000000
    print(time.micros(7).nanos)  # 7000
    print(time.nanos(42).nanos)  # 42
    print(time.to_seconds(time.seconds(2)))  # 2.0
    print(time.to_millis(time.millis(5)))  # 5.0

    # Monotonic clock: sleeping advances it by at least the requested span.
    auto start = time.monotonic_ns()
    time.sleep_for(time.millis(20))
    auto elapsed = time.monotonic_ns() - start
    if elapsed >= time.millis(20).nanos:
        print(1)
    else:
        print(0)

    # Wall clock is well past 2020-01-01T00:00:00Z (unix 1577836800).
    if time.time_ns() > 1577836800000000000:
        print(1)
    else:
        print(0)
