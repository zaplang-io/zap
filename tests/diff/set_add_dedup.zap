# diff: match
# set add + dedup + membership match CPython (epic #374, walled garden). Adding a
# value already present is a no-op, so len counts distinct elements; `in` tests
# membership. Iteration order is unspecified (D-0056), so the sum over elements
# and len/`in` are used — all order-independent — rather than printing the set.
def mark(bool b) -> int64:
    if b:
        return 1
    return 0


def main():
    auto s = set[int64]()
    s.add(1)
    s.add(1)
    s.add(2)
    s.add(3)
    print(len(s))
    print(mark(1 in s))
    print(mark(5 in s))
    auto total = 0
    for x in s:
        total = total + x
    print(total)
