# Golden fixture: deque both-end ops as a generic collection type (#983).
#
# append/appendleft push at the two ends; pop/popleft remove from them; get(i)
# indexes from the left (0 == leftmost). `deque[int64]()` is the generic
# constructor; the old int64-only `deque.new()` module API is gone (D-0208).
def main():
    auto d = deque[int64]()
    d.append(1)
    d.append(2)
    d.appendleft(0)
    # d == [0, 1, 2]
    print(len(d))
    print(d[0])
    print(d[2])
    print(d.popleft())
    print(d.pop())
    print(len(d))
    print(d.get(0))
    d.appendleft(9)
    d.append(8)
    # d == [9, 1, 8]
    print(d.popleft())
    print(d.pop())
    print(d.pop())
    print(len(d))
