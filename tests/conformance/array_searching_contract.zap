# claim: ARR-24
# SPEC §15 (#934): numeric-array searching / selection — own kernels.
# searchsorted(a, v, side=) (binary search, a sorted), unique(a, return_index=/
# return_inverse=/return_counts=) (sorted unique + optional index/inverse/counts
# tuple), argmin/argmax(a, axis=) (first-occurrence; first NaN wins), partition /
# argpartition(a, kth, axis=) (quickselect; a[kth] is the kth order statistic).
# Verified vs NumPy.

def main():
    auto a = array[i64]([1, 3, 3, 5, 7, 9])
    print("ss", str(searchsorted(a, 4)), str(a.searchsorted(5, side="right")))  # 3 4
    auto u = array[i64]([3, 1, 2, 3, 1, 5, 2, 3])
    auto val, cnt = unique(u, return_counts=True)
    print("uni", str(val[0]), str(val[3]), len(val), str(cnt[2]))  # 1 5 4 3
    auto m = array[i64]([5, 3, 8, 8, 1, 8])
    print("am", str(argmin(m)), str(m.argmax()))  # 4 2
    auto bm = array[i64]([[3, 1, 2], [9, -1, 4]])
    auto ax = argmax(bm, axis=1)
    print("amax", str(ax[0]), str(ax[1]))  # 0 0
    auto p = array[i64]([7, 2, 9, 1, 5, 3, 8, 0])
    auto pp = partition(p, 3)
    auto ap = argpartition(p, 3)
    print("part", str(pp[3]), str(p[ap[3]]))  # 3 3
