# diff: match
# Integer arithmetic and bitwise operators within int64 range, where Zap and
# CPython agree (int64 overflow itself is a deliberate divergence — see
# int_overflow.zap). Covers `**`, bit ops, shifts, unary `~`/`-`, and precedence.
def main():
    print(2 ** 10)
    print(3 ** 4)
    print(7 & 3)
    print(5 | 2)
    print(6 ^ 3)
    print(1 << 8)
    print(1024 >> 3)
    print(~5)
    print(~0)
    print(-(3 + 4))
    print(2 + 3 * 4)
    print((2 + 3) * 4)
    print(17 % 5)
    print(100 // 7)
