# diff: match
# Float `%` is floored — it takes the sign of the divisor, consistent with
# floored float `//` — matching CPython (#374, fixed by #751). Mixed-sign
# operands now agree (they used to use C's truncated `frem`): `5.5 % -2` is -0.5,
# `-5.5 % 2` is 0.5. A zero remainder takes the sign of the divisor, so
# `5.0 % -5.0` is -0.0. Same-sign operands are unchanged. Rendered via str().
def main():
    print(str(5.5 % -2))
    print(str(-5.5 % 2))
    print(str(-7.0 % 3.0))
    print(str(7.5 % -2))
    print(str(-4.0 % 2.0))
    print(str(5.0 % -5.0))
    print(str(7.5 % 2))
    print(str(-7.5 % -2))
