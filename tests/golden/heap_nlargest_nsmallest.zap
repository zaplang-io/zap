# Golden fixture: heap.nlargest / heap.nsmallest WITHOUT a key (#978, D-0236) —
# Python heapq semantics. nlargest returns the n largest in descending order,
# nsmallest the n smallest ascending. Edge cases: n > len (whole sequence),
# n == 0 ([]), n < 0 ([]), an empty iterable ([]), and ties/duplicates. str
# (byte order) and float64 (#800 total order) elements too. The source list is
# not modified.
def main():
    auto xs = [5, 1, 3, 2, 4]
    print(heap.nlargest(2, xs))  # [5, 4]
    print(heap.nsmallest(2, xs))  # [1, 2]
    print(heap.nlargest(3, xs))  # [5, 4, 3]
    print(heap.nsmallest(3, xs))  # [1, 2, 3]

    # n > len: the whole sequence, sorted.
    print(heap.nlargest(99, xs))  # [5, 4, 3, 2, 1]
    print(heap.nsmallest(99, xs))  # [1, 2, 3, 4, 5]

    # n == 0 and n < 0: empty.
    print(heap.nlargest(0, xs))  # []
    print(heap.nsmallest(-3, xs))  # []

    # The source list is untouched by the queries.
    print(len(xs))  # 5

    # Empty iterable.
    list[int64] e = []
    print(heap.nlargest(3, e))  # []

    # Ties / duplicates.
    auto ds = [4, 1, 4, 2, 4, 1]
    print(heap.nlargest(3, ds))  # [4, 4, 4]
    print(heap.nsmallest(3, ds))  # [1, 1, 2]

    # str (byte order): apple < cherry < fig < pear.
    auto ss = ["pear", "apple", "cherry", "fig"]
    print(heap.nlargest(2, ss))  # ['pear', 'fig']
    print(heap.nsmallest(2, ss))  # ['apple', 'cherry']

    # float (#800 total order).
    auto fs = [3.5, 1.5, 2.5, 4.5]
    print(heap.nlargest(2, fs))  # [4.5, 3.5]
    print(heap.nsmallest(2, fs))  # [1.5, 2.5]
