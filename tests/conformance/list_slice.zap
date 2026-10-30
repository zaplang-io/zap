# claim: SLICE-1
# SPEC §8 / D-0030: a `list[T]` slice yields a COPY — mutating the slice leaves
# the original untouched — while an `array[T]` slice yields a VIEW that shares
# storage, so a write through either side is visible on the other. Both use
# Python slice bounds (negatives from the end, out-of-range clamps).
def main():
    list[int64] xs = [10, 20, 30, 40, 50]
    auto head = xs[1:3]
    print(len(head))
    print(head[0])
    print(head[1])
    # the copy is independent: writing it does not touch xs
    head[0] = 99
    print(head[0])
    print(xs[1])
    # array slice is a view: a write aliases the parent, and the reverse
    auto a = array[i64]([1, 2, 3, 4, 5])
    auto v = a[1:4]
    print(len(v))
    print(v[0])
    v[0] = 42
    print(a[1])
    a[2] = 77
    print(v[1])
    print(sum(v))
