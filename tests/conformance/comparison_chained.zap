# claim: CMP-1
# SPEC §5 (#386): a chained comparison `a < b < c` has Python semantics — it is
# `(a < b) and (b < c)`, NOT the left-associative `(a < b) < c`. So `3 > 2 > 1`
# is True and `1 < 2 > 3` / `1 < 3 < 2` are False. Rendered through str().
def main():
    print(str(1 < 2 < 3))
    print(str(3 > 2 > 1))
    print(str(1 < 2 > 3))
    print(str(1 < 3 < 2))
