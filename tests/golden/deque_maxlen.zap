# Golden fixture: bounded deque (maxlen ring) (ticket #102).
#
# deque.bounded(n) is a fixed n-slot ring: once full, append drops the leftmost
# element and appendleft drops the rightmost (Python's maxlen semantics).
def main():
    auto d = deque.bounded(3)
    deque.append(d, 1)
    deque.append(d, 2)
    deque.append(d, 3)
    print(deque.len(d))
    deque.append(d, 4)
    # full: 1 dropped -> [2, 3, 4]
    print(deque.get(d, 0))
    print(deque.get(d, 2))
    print(deque.len(d))
    deque.appendleft(d, 0)
    # full: 4 dropped -> [0, 2, 3]
    print(deque.get(d, 0))
    print(deque.get(d, 2))
    print(deque.len(d))
