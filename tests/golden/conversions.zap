# ticket #285 / #282: str/int/float/bool/format conversion builtins. str() is
# used to render bools/floats deterministically (print's own float/bool
# rendering is a separate concern).
def main():
    print(str(42))
    print(str(-7))
    print(str(3.5))
    print(str(True))
    print(str(False))
    print(str("hi"))
    print(int("42"))
    print(int("  -7 "))
    print(str(float("3.14")))
    print(str(float("  2.5 ")))
    print(str(bool(0)))
    print(str(bool(42)))
    print(str(bool("")))
    print(str(bool("x")))
    print(format(255, "x"))
    print(format(255, "08b"))
    print(format(3.14159, ".2f"))
    print(format(42, "+d"))
