# claim: NUM-2
# SPEC §5 (D-0024): `//` is floor division — it rounds toward negative infinity,
# not toward zero (C truncation). The negative-operand cases are the ones that
# distinguish floored from truncated division.
def main():
    print(7 // 2)
    print(-7 // 2)
    print(7 // -2)
    print(-7 // -2)
    print(-1 // 3)
    print(-9 // 4)
