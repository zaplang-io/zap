# Positional-only parameters (#602, epic #614 D10). The bare `/` separator makes
# every parameter before it positional-only: it can only be filled positionally,
# never by name. Parameters after `/` keep accepting positional OR keyword, and
# `/` composes with the `*` keyword-only separator (I#496).

def f(str a, str b, /, str c) -> str:
    # a, b positional-only; c positional-or-keyword.
    return a + b + c

def g(int64 x, /, int64 y, *, int64 z) -> int64:
    # x positional-only; y positional-or-keyword; z keyword-only.
    return x * 100 + y * 10 + z

def main():
    print(f("x", "y", "z"))  # positional-only args filled positionally
    print(f("x", "y", c="z"))  # trailing param supplied by name
    print(str(g(1, 2, z=3)))  # y positional
    print(str(g(1, y=2, z=3)))  # y by name
