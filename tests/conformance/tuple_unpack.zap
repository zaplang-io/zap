# claim: TUP-1, TUP-2, TUP-3
# SPEC §4.8 (D-0049): tuples destructure positionally (TUP-1); the whole RHS is
# evaluated before any target is stored, so the swap idiom `x, y = (y, x)` is
# correct (TUP-2); and multiple return values ARE a tuple return, destructured at
# the call site (TUP-3).
def divmod2(int64 a, int64 b) -> (int64, int64):
    return (a // b, a % b)


def main():
    auto a, b, c = (1, 2, 3)
    print(a + b + c)
    auto x = 10
    auto y = 20
    x, y = (y, x)
    print(x)
    print(y)
    auto q, r = divmod2(17, 5)
    print(q)
    print(r)
