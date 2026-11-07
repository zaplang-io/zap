# diff: match
# set construction + add + dedup + membership match CPython (epic #374, walled
# garden). The constructor deduplicates its input; adding a value already present
# is a no-op while a new value grows the set, so len counts distinct elements and
# `in` tests membership. Iteration order is unspecified (D-0056), so the element
# sum and len/`in` are used — all order-independent — rather than printing the
# set. (The set is built with `set([...])` rather than `set[int64]()`: the latter
# is a typed constructor the Python oracle translator cannot render.)
def mark(bool b) -> int64:
    if b:
        return 1
    return 0


def main():
    auto s = set([1, 1, 2, 3])
    s.add(2)
    s.add(4)
    print(len(s))
    print(mark(4 in s))
    print(mark(5 in s))
    auto total = 0
    for x in s:
        total = total + x
    print(total)
