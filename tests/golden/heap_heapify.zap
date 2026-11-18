# Golden fixture: heap[T](xs) — O(n) heapify (#978, D-0236). Building a heap
# directly from a list and then draining it with pop proves the heapified array
# is a valid heap. Inputs cover empty, single, already-ordered, reverse-sorted,
# and duplicates, for the default min-heap and a max-heap (cmp="max"), plus str
# (byte order) and float64 (#800 total order) elements. heapify copies the words,
# so it leaves the source list untouched.
def drained_min(list[int64] xs) -> list[int64]:
    auto h = heap[int64](xs)
    list[int64] out = []
    while len(h) > 0:
        out.append(h.pop())
    return out

def drained_max(list[int64] xs) -> list[int64]:
    auto h = heap[int64](xs, cmp="max")
    list[int64] out = []
    while len(h) > 0:
        out.append(h.pop())
    return out

def main():
    list[int64] empty = []
    print(len(heap[int64](empty)))  # 0
    print(drained_min([42]))  # [42]
    print(drained_min([1, 2, 3, 4, 5]))  # [1, 2, 3, 4, 5]  already a heap
    print(drained_min([5, 4, 3, 2, 1]))  # [1, 2, 3, 4, 5]  reverse-sorted
    print(drained_min([3, 1, 3, 2, 1, 2]))  # [1, 1, 2, 2, 3, 3]  duplicates
    print(drained_max([5, 1, 3, 2, 4]))  # [5, 4, 3, 2, 1]  max-heap

    # heapify leaves the source list untouched (words are copied, not aliased).
    list[int64] src = [5, 1, 3]
    auto h = heap[int64](src)
    print(len(src))  # 3
    print(src[0])  # 5
    print(h.pop())  # 1

    # str heapify (byte-wise order).
    auto hs = heap[str](["pear", "apple", "cherry"])
    print(hs.pop())  # apple
    print(hs.pop())  # cherry
    print(hs.pop())  # pear

    # float heapify (#800 total order).
    auto hf = heap[float64]([3.5, 1.5, 2.5])
    print(hf.pop())  # 1.5
    print(hf.pop())  # 2.5
    print(hf.pop())  # 3.5
