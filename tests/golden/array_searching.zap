# Golden fixture: numeric-array searching / selection — own kernels (#934, D-0191).
# searchsorted (left/right, scalar or array v), unique (values + return_index /
# return_inverse / return_counts), argmin/argmax (flattened or axis=), partition /
# argpartition (quickselect; the kth element is the kth order statistic). All
# pinned to numpy. argmin/argmax return the first NaN's index if any NaN; unique
# collapses NaNs; partition only pins a[kth] (quickselect order is impl-defined).

def main():
    auto a = array[i64]([1, 3, 3, 5, 7, 9])
    # searchsorted: scalar v -> i64; left/right boundaries.
    print(
        "ss",
        str(searchsorted(a, 4)),
        str(a.searchsorted(5, side="left")),
        str(a.searchsorted(5, side="right")),
    )  # 3 3 4
    # searchsorted: array v -> array[i64].
    auto sv = searchsorted(a, array[i64]([0, 4, 10]))
    print("ssa", str(sv[0]), str(sv[1]), str(sv[2]))  # 0 3 6

    # unique: full (values, index, inverse, counts) as a 4-tuple.
    auto u = array[i64]([3, 1, 2, 3, 1, 5, 2, 3])
    auto val, idx, inv, cnt = unique(
        u,
        return_index=True,
        return_inverse=True,
        return_counts=True,
    )
    print("uval", str(val[0]), str(val[3]), len(val))  # 1 5 4
    print("uidx", str(idx[0]), str(idx[2]))  # 1 0
    print("uinv", str(inv[0]), str(inv[7]))  # 2 2
    print("ucnt", str(cnt[0]), str(cnt[2]))  # 2 3
    # unique: 2-tuple (values, counts).
    auto v2, c2 = unique(u, return_counts=True)
    print("ucnt2", str(v2[3]), str(c2[3]))  # 5 1

    # argmin / argmax: flattened scalar index (first occurrence).
    auto m = array[i64]([5, 3, 8, 8, 1, 8])
    print("am", str(argmin(m)), str(m.argmax()))  # 4 2
    # argmin / argmax along an axis -> rank-(n-1) index array.
    auto bm = array[i64]([[3, 1, 2], [9, -1, 4]])
    print(
        "am_ax",
        str(argmin(bm, axis=0)[0]),
        str(argmin(bm, axis=1)[1]),
        str(argmax(bm, axis=1)[0]),
    )  # 0 1 0

    # partition / argpartition: a[kth] is the kth order statistic.
    auto p = array[i64]([7, 2, 9, 1, 5, 3, 8, 0])
    auto p3 = partition(p, 3)
    print("part", str(p3[3]), str(partition(p, 0)[0]), str(partition(p, 7)[7]))  # 3 0 9
    auto ap = argpartition(p, 3)
    print("argpart", str(p[ap[3]]))  # 3
    auto ps = partition(p, array[i64]([2, 5]))
    print("partseq", str(ps[2]), str(ps[5]))  # 2 7
    # a.partition(kth): in place.
    auto pm = array[i64]([7, 2, 9, 1, 5, 3, 8, 0])
    pm.partition(3)
    print("partip", str(pm[3]))  # 3
