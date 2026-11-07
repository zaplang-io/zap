# diff: match
# Mixed int/float arithmetic promotes the int operand to float, matching CPython
# (epic #374, walled garden). `+ - *` across int and float yield a float; `**`
# with a float exponent goes through libm `pow`. Rendered via str() so the float
# result is visible (`2 ** 0.5` exercises the shortest round-trip repr, B1).
def main():
    print(str(1 + 2.5))
    print(str(3 * 1.5))
    print(str(7 - 1.5))
    print(str(2.0 ** 3))
    print(str(2.0 ** 10))
    print(str(10.0 / 4))
    print(str(4.0 ** 0.5))
    print(str(2 ** 0.5))
