# Golden fixture: ZAP_ACCEL=fast forces the BLAS gemm path for float matmul when
# a provider is linked, regardless of the size floor (#904). In a CI build with
# OpenBLAS this exercises the real cblas hand-off; in a build with no provider,
# `fast` degrades to the own kernel. Either way a ones-product is exactly the
# contraction length, so the result is deterministic. ZAP_ACCEL is set by the
# `.env` sidecar.
def main():
    auto r = ones[f64]((8, 16)) @ ones[f64]((16, 8))  # all 16.0
    print("fast", str(r[0, 0]), str(sum(r)))  # 16.0 1024.0
