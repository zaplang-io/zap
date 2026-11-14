# Golden fixture: vectorized broadcast element-wise kernel (#896). The broadcast
# path (unequal but compatible shapes) walks the result one inner row at a time
# and vectorizes the innermost axis: an operand that is contiguous there is
# vector-loaded, one that is stretched (size-1) there is splat into the lane
# vector. Inner length 10 (a 4-wide body + a 2-element tail) exercises both the
# vector body and the scalar tail; every broadcast SHAPE is covered so no
# specialization (lhs/rhs contiguous-or-splat) can silently corrupt results.
# Results are identical to the scalar reference.
def main():
    auto m = arange[i64](0, 30).reshape(3, 10)  # m[i, j] = 10*i + j
    auto v = arange[i64](0, 10)  # j

    # Row vector: both operands contiguous on the inner axis (vector-load both).
    auto r = m + v  # 10*i + 2*j
    print("R", sum(r), r[0, 0], r[2, 9])  # 570 0 38

    # Column vector: rhs stretched on the inner axis (splat rhs per row).
    auto col = arange[i64](0, 3).reshape(3, 1)
    auto c = m + col  # 11*i + j
    print("C", sum(c), c[0, 0], c[2, 9])  # 465 0 31

    # Outer product: lhs stretched on the inner axis (splat lhs per row).
    auto o = col + v  # (3,1)+(10,) -> (3,10): i + j
    print("O", sum(o), o[0, 0], o[2, 9])  # 165 0 11

    # 3-D, rhs rank-1 contiguous inner (stretched on both leading axes).
    auto t = arange[i64](0, 24).reshape(2, 3, 4)  # 12*i + 4*j + k
    auto vv = arange[i64](0, 4)
    auto t3 = t + vv  # 12*i + 4*j + 2*k
    print("T3", sum(t3), t3[0, 0, 0], t3[1, 2, 3])  # 312 0 26

    # 3-D, rhs stretched on the inner axis AND a leading axis (splat per row).
    auto w = arange[i64](0, 3).reshape(1, 3, 1)
    auto t3c = t + w  # 12*i + 5*j + k
    print("T3c", sum(t3c), t3c[0, 0, 0], t3c[1, 2, 3])  # 300 0 25

    # Mixed dtype (i64 + f64) over a row broadcast -> f64 result.
    auto vf = arange[f64](0, 10)
    auto mf = m + vf
    print("MF", str(sum(mf)), str(mf[2, 9]))  # 570.0 38.0

    # Both operands stretched on the inner axis (inner length 1 -> all tail).
    auto bs = col + array[i64]([[5]])  # (3,1)+(1,1) -> (3,1): i + 5
    print("BS", sum(bs), bs[0, 0], bs[2, 0])  # 18 5 7

    # True division over a broadcast (rhs contiguous inner) -> f64, no raise.
    auto vd = arange[f64](1, 11)  # 1.0 .. 10.0
    auto d = m / vd  # (10*i + j) / (j + 1)
    print("D", str(d[0, 0]), str(d[1, 1]), str(d[2, 4]))  # 0.0 5.5 4.8
