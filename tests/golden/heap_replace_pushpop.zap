# Golden fixture: heapreplace (h.replace) vs heappushpop (h.pushpop) (#978,
# D-0236). Both pop one item and push one, but in OPPOSITE order:
#   replace(x)  — pop the current root FIRST, then push x; returns the old root,
#                 which may be SMALLER than x. Requires a non-empty heap.
#   pushpop(x)  — push x FIRST, then pop; returns the min of {x} ∪ heap, so if x
#                 already outranks the root it is returned unchanged and the heap
#                 is left intact. Defined on an empty heap (returns x).
# They differ exactly when x would be the new root (x smaller than the current
# min, for a min-heap).
def main():
    # replace on a min-heap: root 1 pops, 10 is pushed.
    auto h = heap[int64]([1, 3, 5])
    print(h.replace(10))  # 1  (old root, smaller than 10)
    print(h.peek())  # 3

    # replace with a value smaller than the root still pops the OLD root first.
    auto h2 = heap[int64]([5, 7, 9])
    print(h2.replace(1))  # 5  (old root; 1 is now the new min)
    print(h2.peek())  # 1

    # pushpop where x outranks nothing special (x larger than the root) behaves
    # like replace: push 10, the old min 1 pops out.
    auto p = heap[int64]([1, 3, 5])
    print(p.pushpop(10))  # 1
    print(p.peek())  # 3

    # pushpop where x is SMALLER than the current min returns x unchanged and
    # leaves the heap intact — this is where it differs from replace.
    auto p2 = heap[int64]([5, 7, 9])
    print(p2.pushpop(1))  # 1  (x itself; heap unchanged)
    print(p2.peek())  # 5  (still the original min)
    print(len(p2))  # 3

    # pushpop on an empty heap returns x (replace would abort instead).
    list[int64] e = []
    auto pe = heap[int64](e)
    print(pe.pushpop(42))  # 42
    print(len(pe))  # 0

    # Side-by-side on identical heaps with x below the min: replace returns the
    # old root (5), pushpop returns x (1) — the defining difference, shown plainly.
    auto a = heap[int64]([5, 6, 7])
    auto b = heap[int64]([5, 6, 7])
    print(a.replace(1))  # 5
    print(b.pushpop(1))  # 1

    # Max-heap: replace/pushpop follow the max ordering.
    auto m = heap[int64]([9, 5, 1], cmp="max")
    print(m.replace(3))  # 9  (old max; 3 is pushed)
    print(m.peek())  # 5
    print(m.pushpop(100))  # 100  (x outranks the current max → returned unchanged)
    print(m.peek())  # 5
