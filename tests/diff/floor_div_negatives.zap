# diff: match
# Floor division `//` with negative operands (D-0024): Zap floors toward negative
# infinity, matching CPython (C would truncate toward zero). NOTE: `//` floors
# correctly, but `%` does NOT follow the matching sign rule — that inconsistency
# is a real bug documented in mod_negatives.zap.
def main():
    print(-7 // 2)
    print(7 // -2)
    print(-7 // -2)
    print(-1 // 3)
    print(9 // -4)
    print(-9 // 4)
    print(-8 // 2)
    print(1 // -1)
