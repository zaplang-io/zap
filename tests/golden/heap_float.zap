# Golden fixture: heap[float64] (#800 Phase 1).
#
# A float priority queue: elements are stored as reinterpreted words and ordered
# by IEEE value through a total-order comparator (NaN sorts greatest, so the heap
# invariant holds; see DECISIONS.md D-0137). Covers push out of order, peek/pop
# min-first, a duplicate element, len, non-destructive priority-order iteration,
# and a max-heap via cmp="max". Values format exactly (halves/quarters).
def main():
    auto h = heap[float64]()
    h.push(3.5)
    h.push(1.25)
    h.push(2.0)
    h.push(1.25)  # duplicate
    h.push(4.75)
    print(len(h))  # 5
    print(h.peek())  # 1.25  (min stays in the heap)
    print(h.pop())  # 1.25
    print(h.pop())  # 1.25
    print(h.pop())  # 2.0
    print(len(h))  # 2
    # Iteration yields priority (min-first) order and does not drain the heap.
    float total = 0.0
    for x in h:
        total = total + x
    print(total)  # 3.5 + 4.75 = 8.25
    print(len(h))  # 2

    # Max-heap via the comparator: pops emerge largest-first.
    auto mx = heap[float64](cmp="max")
    mx.push(3.5)
    mx.push(1.25)
    mx.push(2.0)
    print(mx.pop())  # 3.5
    print(mx.pop())  # 2.0
    print(mx.pop())  # 1.25
