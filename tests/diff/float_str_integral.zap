# diff: match
# `str()` of a float renders as a float, matching CPython (I#384). An integral
# value keeps a trailing `.0` (`str(6 / 2)` is "3.0", not "3"), so a float stays
# visibly distinct from an int; non-integral values and the sign are unchanged
# (see division.zap). CPython and Zap both print: 3.0, 6.0, 4.0, -4.0, 0.0, 3.5.
def main():
    print(str(6 / 2))
    print(str(3.0 * 2.0))
    print(str(2.5 + 1.5))
    print(str(-4.0))
    print(str(0.0))
    print(str(3.5))
