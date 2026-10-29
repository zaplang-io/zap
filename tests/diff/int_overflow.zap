# diff: diverge D-0005/D-0019: the default integer is a fixed-width int64 and
# overflow wraps (two's complement). CPython integers are arbitrary precision, so
# the same expression keeps growing. Zap's opt-in `bigint` (D-0019) matches
# CPython, but the default `int` does not. Observable at the int64 boundary:
# CPython 9223372036854775808 / 9223372036854775807+1; Zap wraps negative.
def main():
    print(2 ** 63)
    print(9223372036854775807 + 1)
    print(1000000000000 * 1000000000000)
