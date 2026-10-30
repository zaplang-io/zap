# Golden fixture: array slicing (#400). An array slice is a VIEW (D-0030): it
# shares the parent's storage, so a write through the slice is visible on the
# parent and the reverse. Contiguous only — an array view has no strides yet, so
# a stepped array slice is a documented follow-up (rejected at compile time).
def main():
    auto a = array[i64]([1, 2, 3, 4, 5, 6])
    auto v = a[1:5]
    print(len(v))
    print(v[0])
    print(v[3])
    print(sum(v))
    v[0] = 20
    print(a[1])
    a[2] = 30
    print(v[1])
    print(sum(v))
    auto whole = a[:]
    print(len(whole))
    print(sum(whole))
