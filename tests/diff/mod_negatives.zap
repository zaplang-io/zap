# diff: match
# Integer `%` uses Python's sign-of-divisor (floored) semantics for negative
# operands (I#381, D-0024), consistent with floored `//` (see
# floor_div_negatives.zap), so the identity `a == (a // b) * b + (a % b)` holds:
# e.g. `-7 // 2` is -4 and `-7 % 2` is 1, since -7 == -4*2 + 1.
# CPython and Zap both print: 1, -1, 2, -3.
def main():
    print(-7 % 2)
    print(7 % -2)
    print(-1 % 3)
    print(9 % -4)
