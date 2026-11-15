# Golden fixture: numeric-array sort / argsort — own floor (#932, D-0190).
# `a.sort()` sorts in place; `sort(a)` returns a sorted copy; `argsort` returns
# an i64 index array. `axis=` (default last, or None to flatten) and `kind=`
# (unstable introsort default; "stable" = radix for ints / mergesort for floats).
# NaNs sort to the end. Values match NumPy for any kind; argsort tie-order matches
# NumPy only for the stable kind — so argsort here uses kind="stable" (ties keep
# input order) and the unstable float sort uses distinct values. Pinned to
# numpy.sort / numpy.argsort.
import math

def main():
    # a.sort(): in place, unstable (ties among equal ints are indistinguishable).
    auto a = array[i64]([5, -3, 8, 0, 7, 2])
    a.sort()
    print("inplace", str(a[0]), str(a[1]), str(a[5]))  # -3 0 8

    # sort(a): a new sorted copy (the argument is left unchanged).
    auto c = sort(array[i64]([5, -3, 8, 0, 7, 2]))
    print("copy", str(c[0]), str(c[1]), str(c[2]), str(c[3]), str(c[4]), str(c[5]))  # -3 0 2 5 7 8

    # argsort (stable): ties keep input order, matching NumPy's stable argsort.
    auto ix = argsort(array[i64]([5, -3, 8, -3, 0, 7, -3, 2]), kind="stable")
    print(
        "args",
        str(ix[0]),
        str(ix[1]),
        str(ix[2]),
        str(ix[3]),
        str(ix[4]),
        str(ix[5]),
        str(ix[6]),
        str(ix[7]),
    )  # 1 3 6 4 7 0 5 2

    # float sort (unstable, distinct values) — NaN sorts to the end.
    auto fs = sort(array[f64]([3.0, math.nan, -1.5, 2.0, -7.0]))
    print("fsort", str(fs[0]), str(fs[1]), str(fs[2]), str(fs[3]), str(fs[4]))  # -7.0 -1.5 2.0 3.0 nan

    # float argsort (stable): -0.0 == +0.0 and NaN==NaN are ordered by input index.
    auto fa = argsort(
        array[f64]([3.0, math.nan, -1.5, 0.0, -0.0, 2.0, math.nan, -1.5]),
        kind="stable",
    )
    print(
        "fargs",
        str(fa[0]),
        str(fa[1]),
        str(fa[2]),
        str(fa[3]),
        str(fa[4]),
        str(fa[5]),
        str(fa[6]),
        str(fa[7]),
    )  # 2 7 3 4 5 0 1 6

    # 2-D: sort along an axis, and axis=None to flatten.
    auto m = array[i64]([[3, 1, 2], [9, -1, 4]])
    auto a1 = sort(m, axis=1)
    print(
        "ax1",
        str(a1[0, 0]),
        str(a1[0, 1]),
        str(a1[0, 2]),
        str(a1[1, 0]),
        str(a1[1, 1]),
        str(a1[1, 2]),
    )  # 1 2 3 -1 4 9
    auto a0 = sort(m, axis=0)
    print(
        "ax0",
        str(a0[0, 0]),
        str(a0[0, 1]),
        str(a0[0, 2]),
        str(a0[1, 0]),
        str(a0[1, 1]),
        str(a0[1, 2]),
    )  # 3 -1 2 9 1 4
    auto an = sort(m, axis=None)
    print(
        "axN",
        str(an[0]),
        str(an[1]),
        str(an[2]),
        str(an[3]),
        str(an[4]),
        str(an[5]),
        an.ndim,
    )  # -1 1 2 3 4 9 1
    auto ag = argsort(m, axis=1, kind="stable")
    print(
        "ax1arg",
        str(ag[0, 0]),
        str(ag[0, 1]),
        str(ag[0, 2]),
        str(ag[1, 0]),
        str(ag[1, 1]),
        str(ag[1, 2]),
    )  # 1 2 0 1 2 0

    # other dtypes: uint8 + int32 (radix sign handling via stable kind).
    auto u = sort(array[u8]([255, 0, 128, 1, 7]), kind="stable")
    print("u8", str(u[0]), str(u[1]), str(u[2]), str(u[3]), str(u[4]))  # 0 1 7 128 255
    auto z = sort(array[i32]([2147483647, -2147483648, 0, -1, 1]), kind="stable")
    print("i32", str(z[0]), str(z[1]), str(z[2]), str(z[3]), str(z[4]))  # -2147483648 -1 0 1 2147483647
