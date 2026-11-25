# diff: iter() / next() over a container match CPython exactly (#1382 phase 3) —
# explicit stepping of a list and a range, and the next(it, default) exhaustion
# form. Deterministic by construction: sequences only, no dict/set iteration
# order (which Zap leaves unspecified, D-1066).
def main():
    auto it = iter([10, 20, 30])
    print(next(it))
    print(next(it))
    print(next(it))
    print(next(it, -1))
    auto r = iter(range(3))
    print(next(r))
    print(next(r))
    print(next(r))
    print(next(r, 99))
    auto total = 0
    for x in iter([1, 2, 3, 4]):
        total = total + x
    print(total)
    auto first = next(iter([7, 8, 9]))
    print(first + 1)
