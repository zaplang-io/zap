# claim: AUGSUB-1
# SPEC §3 (#374, #750): augmented assignment to a field or subscript target is a
# read-modify-write of that single slot; a side-effecting subscript index is
# evaluated exactly once.
struct Counter:
    int64 n


def bump() -> int64:
    print("bump")
    return 0


def main():
    Counter c = Counter(10)
    c.n += 5
    print(c.n)
    c.n *= 2
    print(c.n)
    c.n -= 1
    print(c.n)
    list[int64] xs = [1, 2, 3]
    xs[bump()] += 100
    print(xs[0])
    print(xs[1])
