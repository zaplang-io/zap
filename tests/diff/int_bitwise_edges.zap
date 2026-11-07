# diff: match
# Bitwise and shift edge cases within int64 range, where Zap and CPython agree
# (epic #374, walled garden). Shift counts are literals in 0..63 and no result
# crosses the int64 boundary, so overflow (a deliberate divergence, see
# int_overflow.zap) is not in play. `~(-1) == 0` and `-1 & 0xFF == 255` pin the
# two's-complement behavior of `~` and `&` on a negative operand.
def main():
    print(255 & 0)
    print(0xFF ^ 0xFF)
    print(1 << 62)
    print(1 << 62 >> 60)
    print(~-1)
    print(-1 & 0xFF)
    print(12345 | 0)
    print(0 << 5)
    print(0xF0 >> 4)
    print((1 << 10) - 1)
