# diff: diverge BUG: the `%` operator does not use Python's sign-of-divisor
# (floored) semantics for negative operands — it uses C-style truncated
# remainder (sign of the dividend). This is INCONSISTENT with `//`, which DOES
# floor (see floor_div_negatives.zap), so the identity `a == (a // b) * b +
# (a % b)` is broken: e.g. `-7 // 2` is -4 but `-7 % 2` is -1 (should be 1, since
# -7 == -4*2 + 1). A real bug to triage (D-0024 says modulo is Python-style).
# CPython: 1, -1, 2, -3; Zap: -1, 1, -1, 1.
def main():
    print(-7 % 2)
    print(7 % -2)
    print(-1 % 3)
    print(9 % -4)
