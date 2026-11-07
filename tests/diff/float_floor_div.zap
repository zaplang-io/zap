# diff: match
# Float floor division `//` (D-0024) floors toward negative infinity for every
# sign combination, matching CPython (`zap_ffloordiv == floor(a/b)`). Float `%`
# matches CPython when the operands share a sign (the mixed-sign case diverges —
# that is a real bug, pinned in float_mod_negatives.zap). Rendered via str() so
# the float result (with its `.0`) is visible.
def main():
    print(str(-7.5 // 2))
    print(str(7.5 // 2))
    print(str(-7.5 // -2))
    print(str(7.5 // -2))
    print(str(9.0 // 2.0))
    print(str(-9.0 // 2.0))
    print(str(7.5 % 2))
    print(str(9.0 % 2.0))
    print(str(-7.5 % -2))
    print(str(0.0 // 1.0))
