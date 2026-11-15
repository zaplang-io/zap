# claim: ARR-23
# SPEC §15 (#932): numeric-array sort/argsort — own floor. `a.sort()` in place,
# `sort(a)` a sorted copy, `a.argsort()`/`argsort(a)` an i64 index array; `axis=`
# (default last, None flattens) and `kind=` (unstable introsort default; "stable"
# = radix for ints / mergesort for floats). NaNs sort to the end; values match
# NumPy for any kind, argsort tie-order for the stable kind. Verified vs NumPy.
import math

def main():
    auto c = sort(array[i64]([5, -3, 8, 0, 7, 2]))
    print("sorted", str(c[0]), str(c[1]), str(c[2]), str(c[3]), str(c[4]), str(c[5]))  # -3 0 2 5 7 8
    auto ix = argsort(array[i64]([5, -3, 8, -3, 0, 7, -3, 2]), kind="stable")
    print("args", str(ix[0]), str(ix[1]), str(ix[3]), str(ix[7]))  # 1 3 4 2
    auto fs = sort(array[f64]([3.0, math.nan, -1.5, 2.0]))
    print("fnan", str(fs[0]), str(fs[1]), str(fs[2]), str(fs[3]))  # -1.5 2.0 3.0 nan
    auto m = array[i64]([[3, 1, 2], [9, -1, 4]])
    auto a1 = sort(m, axis=1)
    print("ax1", str(a1[0, 0]), str(a1[0, 2]), str(a1[1, 0]))  # 1 3 -1
    auto an = sort(m, axis=None)
    print("axN", str(an[0]), str(an[5]), an.ndim)  # -1 9 1
    auto z = sort(array[i32]([2147483647, -2147483648, 0, -1, 1]), kind="stable")
    print("i32", str(z[0]), str(z[1]), str(z[4]))  # -2147483648 -1 2147483647
