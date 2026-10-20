# Golden fixture: bisect search + insort over a sorted list[int64] (ticket #102).
#
# bisect_left/right give the insertion point (left/right of equal elements);
# insort_left/right insert in place keeping the list sorted.
def main():
    auto xs = [1, 3, 5, 7]
    print(bisect.bisect_left(xs, 5))
    print(bisect.bisect_right(xs, 5))
    print(bisect.bisect_left(xs, 4))
    print(bisect.bisect_left(xs, 0))
    print(bisect.bisect_right(xs, 9))
    bisect.insort_left(xs, 4)
    # xs == [1, 3, 4, 5, 7]
    bisect.insort_right(xs, 5)
    # xs == [1, 3, 4, 5, 5, 7]
    print(len(xs))
    print(xs[2])
    print(xs[3])
    print(xs[4])
    print(bisect.bisect_left(xs, 5))
    print(bisect.bisect_right(xs, 5))
