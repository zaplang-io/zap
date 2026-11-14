# claim: ARR-15
# SPEC §15 (#913): the matrix-multiply floor kernel (`@` / `dot`, #509/D-0172) is
# vectorized (contiguous `ikj` order over materialized buffers) instead of a
# scalar column-walking triple loop — a performance change only, with NO effect
# on results. Per output cell the contraction keeps its order, so products are
# bit-identical to the scalar reference for every dtype and shape.
def main():
    print("ones", sum(ones[i64]((8, 16)) @ ones[i64]((16, 8))))  # 1024
    auto p = arange[i64](0, 12).reshape(3, 4) @ ones[i64]((4, 2))
    print("rowsum", p[0, 0], p[2, 1], sum(p))  # 6 38 132
    auto q = ones[i64]((2, 4)) @ arange[i64](0, 12).reshape(4, 3)
    print("colsum", q[0, 0], q[0, 2])  # 18 26
    auto mx = arange[i64](0, 12).reshape(3, 4) @ ones[f64]((4, 2))
    print("mixed", str(sum(mx)))  # 132.0
