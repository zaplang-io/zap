# Golden fixture: integer `%` uses Python floored modulo (I#381, D-0024) — the
# result takes the sign of the divisor, consistent with floored `//`. The four
# sign combinations below match CPython, and the identity
# `a == (a // b) * b + (a % b)` holds for each pair.
def main():
    # (-, +), (+, -), (-, +), (+, -): result has the sign of the divisor.
    print(-7 % 2)
    print(7 % -2)
    print(-1 % 3)
    print(9 % -4)
    # Plain positive case is unchanged.
    print(7 % 3)
    # Identity `a == (a // b) * b + (a % b)` across every sign combination.
    print(-7 // 2 * 2 + -7 % 2)
    print(7 // -2 * -2 + 7 % -2)
    print(-1 // 3 * 3 + -1 % 3)
    print(9 // -4 * -4 + 9 % -4)
    print(7 // 3 * 3 + 7 % 3)
