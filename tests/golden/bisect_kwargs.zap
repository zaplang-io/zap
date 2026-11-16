# Golden fixture: bisect keyword & reordered argument binding (#998, D-0204).
#
# bisect_left/right and insort_left/right take Python-style keyword arguments
# (a, x) in any order. A reordered call (`x=` before `a=`) must still place the
# list in slot 0 and the value in slot 1 — the central seam reorder runs before
# bisect's per-method lowering, which reads purely by position and mutates the
# list in place for insort. Results must match the positional golden exactly.
def main():
    auto xs = [1, 3, 5, 7]
    print(bisect.bisect_left(a=xs, x=5))
    # reordered: value before list
    print(bisect.bisect_right(x=5, a=xs))
    print(bisect.bisect_left(x=4, a=xs))
    # insort by keyword (in-place mutation of the reordered slot-0 list)
    bisect.insort_left(a=xs, x=4)
    # xs == [1, 3, 4, 5, 7]
    bisect.insort_right(x=5, a=xs)
    # xs == [1, 3, 4, 5, 5, 7]
    print(len(xs))
    print(xs[2])
    print(bisect.bisect_left(a=xs, x=5))
    print(bisect.bisect_right(x=5, a=xs))
