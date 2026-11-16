# Golden fixture: bounded deque (maxlen ring) as a generic collection (#983).
#
# `deque[T](maxlen=n)` is a fixed n-slot ring: once full, append drops the
# leftmost element and appendleft drops the rightmost (Python's maxlen
# semantics). Replaces the old int64-only `deque.bounded(n)` module API (D-0208).
def main():
    auto d = deque[int64](maxlen=3)
    d.append(1)
    d.append(2)
    d.append(3)
    print(len(d))
    d.append(4)
    # full: 1 dropped -> [2, 3, 4]
    print(d.get(0))
    print(d.get(2))
    print(len(d))
    d.appendleft(0)
    # full: 4 dropped -> [0, 2, 3]
    print(d.get(0))
    print(d.get(2))
    print(len(d))
