# Golden fixture: deque both-end ops (ticket #102).
#
# append/appendleft push at the two ends; pop/popleft remove from them; get(i)
# indexes from the left (0 == leftmost). Exercises a grow (cap doubles past the
# initial 4) is not needed here — a small deque suffices to cover both ends.
def main():
    auto d = deque.new()
    deque.append(d, 1)
    deque.append(d, 2)
    deque.appendleft(d, 0)
    # d == [0, 1, 2]
    print(deque.len(d))
    print(deque.get(d, 0))
    print(deque.get(d, 2))
    print(deque.popleft(d))
    print(deque.pop(d))
    print(deque.len(d))
    print(deque.get(d, 0))
    deque.appendleft(d, 9)
    deque.append(d, 8)
    # d == [9, 1, 8]
    print(deque.popleft(d))
    print(deque.pop(d))
    print(deque.pop(d))
    print(deque.len(d))
