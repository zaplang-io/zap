# Golden fixture: heap[int64] — the built-in min-heap priority queue (D-0057).
# Push values out of order, then peek/pop to show they emerge smallest-first;
# len; priority-order (non-destructive) iteration; and a max-heap via cmp="max".
def main():
    auto h = heap[int64]()
    h.push(5)
    h.push(1)
    h.push(3)
    h.push(2)
    h.push(4)
    print(len(h))  # 5
    print(h.peek())  # 1  (min stays in the heap)
    print(h.pop())  # 1
    print(h.pop())  # 2
    print(h.pop())  # 3
    print(len(h))  # 2
    # Iteration yields priority (min-first) order and does not drain the heap.
    auto total = 0
    for x in h:
        total = total + x
    print(total)  # 4 + 5 = 9
    print(len(h))  # 2

    # Max-heap via the comparator: pops emerge largest-first.
    auto mx = heap[int64](cmp="max")
    mx.push(5)
    mx.push(1)
    mx.push(3)
    print(mx.pop())  # 5
    print(mx.pop())  # 3
    print(mx.pop())  # 1
