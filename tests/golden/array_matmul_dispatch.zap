# Golden fixture: automatic matmul dispatch (#904). `a @ b` picks the own
# blocked+SIMD floor (#913) or a BLAS gemm by (dtype, size, ZAP_ACCEL): integers
# always own (exact); small float -> own; large float -> BLAS when a provider is
# linked (auto past the floor). Results must be the same whichever kernel runs,
# so these use values that are exact under any order — a ones-product is exactly
# the contraction length, and an integer product is exact — so the fixture is
# deterministic whether or not BLAS is present in the build.
def main():
    # Integer: always the own kernel (BLAS routes ints through f64). 64x64 ones.
    auto bi = ones[i64]((64, 64)) @ ones[i64]((64, 64))  # all 64
    print("intbig", sum(bi))  # 64*64*64 = 262144

    # Small float: below the dispatch floor -> own kernel (deterministic).
    auto fs = arange[f64](0, 12).reshape(3, 4) @ ones[f64]((4, 2))  # row sums
    print("fsmall", str(fs[0, 0]), str(fs[2, 0]), str(sum(fs)))  # 6.0 38.0 132.0

    # Large float: at/above the floor -> BLAS when linked (else own). Ones-product
    # is exactly the contraction length under either kernel.
    auto fb = ones[f64]((64, 64)) @ ones[f64]((64, 64))  # all 64.0
    print("fbig", str(fb[0, 0]), str(sum(fb)))  # 64.0 262144.0
