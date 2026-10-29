# diff: match
# Multiple assignment / tuple unpacking and the swap idiom (D-0049). The
# right-hand side is fully evaluated before any target is stored, so the swap is
# correct — the same as CPython.
def main():
    auto a, b, c = (1, 2, 3)
    print(a)
    print(b)
    print(c)
    auto x = 7
    auto y = 9
    x, y = (y, x)
    print(x)
    print(y)
    auto p, q = (q_seed(), p_seed())
    print(p)
    print(q)


def q_seed() -> int64:
    return 11


def p_seed() -> int64:
    return 22
