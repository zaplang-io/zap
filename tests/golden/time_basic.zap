# Golden fixture: the `time` module (ticket #99, D-0056).
#
# Durations are plain int64 nanoseconds, so unit construction/conversion and
# arithmetic are exact and deterministic. The clock checks are RELATIVE
# (monotonic elapsed across a sleep, wall clock past a fixed past instant), so
# the output is identical on every run and under both collectors.

def main():
    # Duration units compose with ordinary int64 arithmetic.
    auto d = time.seconds(1) + time.millis(500)  # 1_500_000_000 ns
    print(d)  # 1500000000
    print(time.millis(250))  # 250000000
    print(time.micros(7))  # 7000
    print(time.nanos(42))  # 42
    print(time.to_seconds(time.seconds(2)))  # 2.000000
    print(time.to_millis(time.millis(5)))  # 5.000000

    # Monotonic clock: sleeping advances it by at least the requested span.
    auto start = time.monotonic_ns()
    time.sleep_ns(time.millis(20))
    auto elapsed = time.monotonic_ns() - start
    if elapsed >= time.millis(20):
        print(1)
    else:
        print(0)

    # Wall clock is well past 2020-01-01T00:00:00Z (unix 1577836800).
    if time.time_ns() > 1577836800000000000:
        print(1)
    else:
        print(0)
