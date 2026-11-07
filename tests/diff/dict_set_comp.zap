# diff: match
# dict and set comprehensions match CPython (epic #374, walled garden). A dict
# comprehension with colliding keys keeps the LAST value written; a set
# comprehension deduplicates. Results are probed by specific key lookups and by
# len / membership (all order-independent, since dict/set order is unspecified,
# D-0056). CPython is the oracle.
def mark(bool b) -> int64:
    if b:
        return 1
    return 0


def main():
    auto d = {k % 3: k for k in [0, 1, 2, 3, 4, 5, 6]}
    print(d[0])
    print(d[1])
    print(d[2])
    auto sq = {x * x for x in [1, 2, 2, 3]}
    print(len(sq))
    print(mark(4 in sq))
    print(mark(5 in sq))
