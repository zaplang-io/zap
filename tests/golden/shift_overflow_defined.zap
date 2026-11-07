# Golden (epic #374, #740): a shift count >= the int64 width is defined under
# wrapping int64, not LLVM poison. `1 << 64` shifts all bits out -> 0; arithmetic
# `-1 >> 100` saturates to the sign bit -> -1. Runtime counts force the guarded
# helper (zap_ishl/zap_ishr); CPython would use bigint for `1 << 64`.
def main():
    int64 big = 64
    int64 wide = 100
    int64 neg = -1
    print(1 << big)
    print(neg >> wide)
