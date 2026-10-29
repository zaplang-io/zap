# claim: NUM-4
# SPEC §4.1 (D-0005, D-0019): the default `int` is a fixed-width signed int64 and
# overflow WRAPS (two's complement). This is a DELIBERATE divergence from Python,
# whose ints are arbitrary precision (Zap's opt-in `bigint` is the unbounded
# type). Observable at the int64 upper boundary: adding past i64::MAX wraps to a
# negative, and `2 ** 63` (one past the max) is the most-negative int64.
def main():
    print(9223372036854775807 + 1)
    print(9223372036854775807 + 2)
    print(2 ** 63)
