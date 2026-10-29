# claim: NUM-3
# SPEC §5 (#381, D-0024): `%` is floored modulo — its result takes the sign of
# the DIVISOR (Python), consistent with floored `//`, so the identity
# `a == (a // b) * b + (a % b)` holds. (C-truncated `%` would take the sign of
# the dividend; that was the #381 bug.)
def main():
    print(-7 % 2)
    print(7 % -2)
    print(-1 % 3)
    print(9 % -4)
    print(-9 % -4)
