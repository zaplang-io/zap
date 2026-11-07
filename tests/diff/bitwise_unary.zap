# diff: match
# Bitwise and unary operators on negatives match CPython (epic #374, walled
# garden): `~x == -x-1`, floored `%` takes the divisor's sign, and an in-range
# arithmetic `>>` sign-extends. (Shift counts here are literals in 0..63.)
def main():
    print(~5)
    print(~0)
    print(--3)
    print(5 % -3)
    print(-5 % 3)
    print(5 & -1)
    print(-1 >> 1)
    print(1 << 10)
