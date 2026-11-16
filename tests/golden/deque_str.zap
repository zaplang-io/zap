# Golden fixture: a deque of strings + iteration + bounded eviction (#983).
#
# The re-skinned `deque[T]` is generic over its element type, so a `deque[str]`
# stores pointer (string) elements — GC-traced via the ZAP_TD_DEQUE arm — and
# `for x in d` walks a left-to-right snapshot. Bounded eviction works for any T.
def main():
    auto d = deque[str]()
    d.append("b")
    d.append("c")
    d.appendleft("a")
    # d == [a, b, c]
    print(len(d))
    print(d.get(0))
    print(d.get(2))
    for x in d:
        print(x)
    print(d.popleft())
    print(d.pop())
    print(len(d))
    d.clear()
    print(len(d))
    # bounded: a 2-slot ring evicts the far end on overflow
    auto q = deque[str](maxlen=2)
    q.append("x")
    q.append("y")
    q.append("z")
    # full: "x" dropped -> [y, z]
    print(q.get(0))
    print(q.get(1))
    print(len(q))
