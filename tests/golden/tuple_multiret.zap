# Golden fixture: multiple return IS tuple return (D-0049) — no separate
# mechanism. The caller may destructure the returned tuple, or bind it whole and
# index it by a constant.
def divmod(int64 a, int64 b) -> (int64, int64):
    return a // b, a % b

def main():
    auto (q, r) = divmod(17, 5)
    print(q)
    print(r)
    auto both = divmod(20, 6)
    print(both[0])
    print(both[1])
