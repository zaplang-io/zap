# diff: match
# Chained comparison `a < b < c` follows Python semantics (I#386): it is
# `(a < b) and (b < c)` with each operand evaluated once, left to right, not the
# left-associative `(a < b) < c`. So `1 < 3 < 2` is False, `3 > 2 > 1` is True,
# and `2 < 1 < 3` is False — identical to CPython. Rendered via str().
def main():
    print(str(1 < 3 < 2))
    print(str(3 > 2 > 1))
    print(str(2 < 1 < 3))
