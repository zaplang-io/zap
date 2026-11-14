# claim: ARR-14
# SPEC §15 (#896): the broadcast element-wise kernel vectorizes its innermost
# axis (contiguous operand -> vector-load, stretched operand -> splat) instead
# of a per-element scalar loop. A performance change only, with NO effect on
# results: a broadcast of any shape reduces to the same values the scalar
# reference computes. This contract pins a row vector, a column vector, an outer
# product, a 3-D broadcast, and a mixed-dtype broadcast.
def main():
    auto m = arange[i64](0, 30).reshape(3, 10)  # 10*i + j
    print("row", sum(m + arange[i64](0, 10)))  # 570
    auto col = arange[i64](0, 3).reshape(3, 1)
    print("colouter", sum(m + col), sum(col + arange[i64](0, 10)))  # 465 165
    auto t = arange[i64](0, 24).reshape(2, 3, 4)
    print("nd", sum(t + arange[i64](0, 4)), sum(t + arange[i64](0, 3).reshape(1, 3, 1)))  # 312 300
    auto mf = m + arange[f64](0, 10)
    print("mixed", str(sum(mf)), str(mf[2, 9]))  # 570.0 38.0
