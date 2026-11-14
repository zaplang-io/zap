# Golden fixture: the `linalg` module — solve / inv / det (#926, D-0183),
# LAPACK-backed. The matrices here are diagonal (powers of two) or unit
# upper-triangular, so the LU is trivial and every result is exact in IEEE-754
# (no rounding) and identical across LAPACK providers. Integer input promotes to
# f64; a singular matrix raises a catchable ValueError. (Needs a BLAS/LAPACK
# provider — the CI image ships one.)
import linalg

def main():
    # det: exact integer products (and integer input -> f64).
    print("det", str(linalg.det(array[f64]([[1.0, 2.0], [3.0, 4.0]]))))  # -2.0
    print("detd", str(linalg.det(array[f64]([[2.0, 0.0], [0.0, 4.0]]))))  # 8.0
    print("deti", str(linalg.det(array[i64]([[1, 2], [3, 4]]))))  # -2.0

    # solve, 1-D b: diagonal (powers of two) and unit upper-triangular -> exact.
    auto xs = linalg.solve(array[f64]([[2.0, 0.0], [0.0, 4.0]]), array[f64]([6.0, 8.0]))
    print("solve", str(xs[0]), str(xs[1]))  # 3.0 2.0
    auto xt = linalg.solve(array[f64]([[1.0, 1.0], [0.0, 1.0]]), array[f64]([3.0, 1.0]))
    print("solvet", str(xt[0]), str(xt[1]))  # 2.0 1.0

    # solve, 2-D b (each column a separate RHS): A=diag(2,4), B cols [2,4],[6,8].
    auto xm = linalg.solve(
        array[f64]([[2.0, 0.0], [0.0, 4.0]]),
        array[f64]([[2.0, 6.0], [4.0, 8.0]]),
    )
    print("solvem", str(xm[0, 0]), str(xm[0, 1]), str(xm[1, 0]), str(xm[1, 1]))  # 1.0 3.0 1.0 2.0

    # inv: diagonal (powers of two) and unit upper-triangular -> exact.
    # (The off-diagonal of a diagonal inverse is a computed zero whose SIGN is
    # LAPACK-impl-dependent — OpenBLAS yields -0.0 — so only the diagonal, which
    # is exact, is pinned here.)
    auto iv = linalg.inv(array[f64]([[2.0, 0.0], [0.0, 4.0]]))
    print("inv", str(iv[0, 0]), str(iv[1, 1]))  # 0.5 0.25
    auto iu = linalg.inv(array[f64]([[1.0, 1.0], [0.0, 1.0]]))
    print("invu", str(iu[0, 0]), str(iu[0, 1]), str(iu[1, 1]))  # 1.0 -1.0 1.0

    # singular matrix -> catchable ValueError.
    try:
        auto bad = linalg.inv(array[f64]([[1.0, 2.0], [2.0, 4.0]]))
        print(str(bad[0, 0]))
    except ValueError:
        print("singular raises")
