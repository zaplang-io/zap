# Golden fixture: the blocked+SIMD matmul floor kernel (#913). Larger products
# than array_matmul — a contraction axis (k) and result inner dim (n) that cross
# the vector width, with tails — exercise the vectorized `ikj` inner loop and its
# scalar remainder. Results are bit-identical to the scalar reference, so these
# are plain known values (ones-products give a constant; arange-with-ones gives
# row/column sums).
def main():
    # ones(m,k) @ ones(k,n) -> every element is k. k=16 and k=20 cross the
    # vector width; n=8 and n=5 exercise a full-width body and a tail.
    auto r1 = ones[i64]((8, 16)) @ ones[i64]((16, 8))  # all 16
    print("ones", sum(r1), sum(ones[i64]((3, 20)) @ ones[i64]((20, 5))))  # 1024 300

    auto rf = ones[f64]((8, 16)) @ ones[f64]((16, 8))  # all 16.0
    print("onesf", str(sum(rf)), str(sum(ones[f64]((5, 32)) @ ones[f64]((32, 4)))))  # 1024.0 640.0

    # arange(m,k) @ ones(k,n) -> each output cell is the row sum of the arange
    # matrix (varying values catch an index/order bug the ones-product can't).
    auto p = arange[i64](0, 12).reshape(3, 4) @ ones[i64]((4, 2))
    print("rowsum", p[0, 0], p[2, 1], sum(p))  # 6 38 132

    # ones(m,k) @ arange(k,n) -> each output cell is a column sum.
    auto q = ones[i64]((2, 4)) @ arange[i64](0, 12).reshape(4, 3)
    print("colsum", q[0, 0], q[0, 2], sum(q))  # 18 26 132

    # mixed dtype (i64 @ f64 -> f64): row sums as floats.
    auto mx = arange[i64](0, 12).reshape(3, 4) @ ones[f64]((4, 2))
    print("mixed", str(mx[0, 0]), str(mx[1, 0]), str(mx[2, 0]))  # 6.0 22.0 38.0
