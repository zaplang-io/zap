# claim: ARR-19
# SPEC §15 (#928): `import linalg` eigensolvers / least squares / norms / pinv /
# matrix_power. eigvals(a) -> list[complex] (general; conjugate pairs with +imag
# first); eig deferred (no complex array dtype). eigh(a) -> (w ascending, v),
# eigvalsh(a) -> w (symmetric, real). lstsq(a, b) -> (x, residuals, rank, s).
# norm(a[, ord]) -> f64 (default 2-norm / Frobenius; integer/inf ords). pinv(a)
# via SVD. matrix_power(a, p). Results f64; eigenvector / SVD-solution gauge is
# ambiguous, so this pins the well-defined facts.
import linalg
import math

def main():
    auto ev = linalg.eigvals(array[f64]([[0.0, -1.0], [1.0, 0.0]]))
    print("eigvals", str(ev))  # [(0+1j), (0-1j)]

    auto w, v = linalg.eigh(array[f64]([[2.0, 0.0], [0.0, 3.0]]))
    print("eigh", str(w[0]), str(w[1]), v.shape[0], v.shape[1])  # 2.0 3.0 2 2
    auto wv = linalg.eigvalsh(array[f64]([[2.0, 0.0], [0.0, 3.0]]))
    print("eigvalsh", str(wv[0]), str(wv[1]))  # 2.0 3.0

    auto x, resid, rank, s = linalg.lstsq(
        array[f64]([[1.0, 1.0], [1.0, 2.0], [1.0, 3.0]]),
        array[f64]([6.0, 5.0, 7.0]),
    )
    print("lstsq", len(x), rank, len(resid), len(s))  # 2 2 1 2

    auto vn = array[f64]([3.0, -4.0, 0.0])
    print(
        "norm",
        str(linalg.norm(vn)),
        str(linalg.norm(vn, 1.0)),
        str(linalg.norm(vn, math.inf)),
    )  # 5.0 7.0 4.0

    auto pv = linalg.pinv(array[f64]([[1.0, 0.0], [0.0, 1.0], [0.0, 0.0]]))
    print("pinv", pv.shape[0], pv.shape[1], str(pv[0, 0]))  # 2 3 1.0

    auto mp = linalg.matrix_power(array[f64]([[2.0, 0.0], [0.0, 1.0]]), 3)
    print("mpow", str(mp[0, 0]), str(mp[1, 1]))  # 8.0 1.0
