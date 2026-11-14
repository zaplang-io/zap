# claim: ARR-16
# SPEC §15 (#904): `a @ b` automatically dispatches to the fastest kernel for the
# (dtype, size, CPU budget) — the own blocked+SIMD floor (#913) or a BLAS gemm
# (#912 seam) for large float products — with a ZAP_ACCEL override. Integers are
# always computed by the own kernel (exact; BLAS routes ints through f64). The
# dispatch is a performance choice only: for a given product every kernel yields
# the same result (within fp tolerance for BLAS float; this contract pins exact
# cases — integer products and float ones-products — so it holds for any build).
def main():
    print("int", sum(ones[i64]((64, 64)) @ ones[i64]((64, 64))))  # 262144
    auto fs = arange[f64](0, 12).reshape(3, 4) @ ones[f64]((4, 2))
    print("small", str(sum(fs)))  # 132.0
    print("big", str(sum(ones[f64]((64, 64)) @ ones[f64]((64, 64)))))  # 262144.0
