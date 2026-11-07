# diff: match
# Numeric conversions match CPython (epic #374, walled garden). `int(float)`
# truncates toward zero (NOT floored) — so `int(-3.9)` is -3, not -4 — and
# `float(int)` widens exactly. int() results print as ints; float() results are
# rendered via str() so the trailing `.0` is visible.
def main():
    print(int(3.9))
    print(int(-3.9))
    print(int(2.9))
    print(int(-2.1))
    print(int(0.5))
    print(str(float(5)))
    print(str(float(-3)))
    print(str(float(0)))
