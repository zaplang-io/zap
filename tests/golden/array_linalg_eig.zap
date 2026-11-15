# Golden fixture: linalg eigensolvers / least squares / norms / pinv /
# matrix_power (#928), LAPACK-backed. Eigenvectors (eigh v) and the SVD-computed
# lstsq solution are gauge-ambiguous / fp-dependent, so the deterministic,
# provider-portable facts are pinned here: eigvals of a rotation matrix (an exact
# conjugate pair, LAPACK yields +imag first), symmetric eigenvalues of a diagonal
# matrix (exact, ascending), exact integer/inf norms, the orthonormal-column
# pinv, and matrix_power of a diagonal matrix. Numeric reconstruction of the
# ambiguous pieces is verified by the runtime's host checks. (Needs a BLAS/LAPACK
# provider.)
import linalg
import math

def main():
    # eigvals (general): the rotation matrix [[0,-1],[1,0]] has eigenvalues ±i.
    # They come back as a list[complex] (a new element kind). LAPACK orders a
    # conjugate pair with the +imag member first, so the order is deterministic.
    auto ev = linalg.eigvals(array[f64]([[0.0, -1.0], [1.0, 0.0]]))
    print("eigvals", str(ev), len(ev))  # [(0+1j), (0-1j)] 2
    print("eig0", str(ev[0]))  # (0+1j)

    # eigh / eigvalsh (symmetric): real eigenvalues, ascending. A diagonal matrix
    # gives exact eigenvalues (the diagonal, sorted); the eigenvector signs are
    # gauge-ambiguous, so only v's shape is pinned.
    auto w, v = linalg.eigh(array[f64]([[2.0, 0.0], [0.0, 3.0]]))
    print("eigh", str(w[0]), str(w[1]), v.shape[0], v.shape[1])  # 2.0 3.0 2 2
    auto wv = linalg.eigvalsh(array[f64]([[2.0, 0.0], [0.0, 3.0]]))
    print("eigvalsh", str(wv[0]), str(wv[1]))  # 2.0 3.0

    # lstsq (over-determined fit): the solution is SVD-computed (fp), so the pinned
    # facts are the deterministic ones — solution shape, effective rank, singular
    # values count, and that residuals are present (m>n and full column rank).
    auto x, resid, rank, s = linalg.lstsq(
        array[f64]([[1.0, 1.0], [1.0, 2.0], [1.0, 3.0]]),
        array[f64]([6.0, 5.0, 7.0]),
    )
    print("lstsq", x.ndim, len(x), rank, len(resid), len(s))  # 1 2 2 1 2

    # norm: exact integer / inf ords (no ord -> 2-norm / Frobenius).
    auto vn = array[f64]([3.0, -4.0, 0.0])
    print(
        "vnorm",
        str(linalg.norm(vn)),
        str(linalg.norm(vn, 1.0)),
        str(linalg.norm(vn, math.inf)),
    )  # 5.0 7.0 4.0
    auto mn = array[f64]([[1.0, 2.0], [3.0, 4.0]])
    print("mnorm", str(linalg.norm(mn, 1.0)), str(linalg.norm(mn, math.inf)))  # 6.0 7.0

    # pinv: a (3x2) with orthonormal columns -> exact (2x3) pseudo-inverse.
    auto pv = linalg.pinv(array[f64]([[1.0, 0.0], [0.0, 1.0], [0.0, 0.0]]))
    print("pinv", pv.shape[0], pv.shape[1], str(pv[0, 0]), str(pv[1, 1]))  # 2 3 1.0 1.0

    # matrix_power: diagonal -> exact powers; p=0 -> identity; p<0 -> inverse.
    auto mp = linalg.matrix_power(array[f64]([[2.0, 0.0], [0.0, 1.0]]), 3)
    print("mpow", str(mp[0, 0]), str(mp[1, 1]))  # 8.0 1.0
    auto mi = linalg.matrix_power(array[f64]([[2.0, 0.0], [0.0, 4.0]]), -1)
    print("mpowinv", str(mi[0, 0]), str(mi[1, 1]))  # 0.5 0.25
    auto m0 = linalg.matrix_power(array[f64]([[5.0, 7.0], [2.0, 3.0]]), 0)
    print("mpow0", str(m0[0, 0]), str(m0[0, 1]), str(m0[1, 0]), str(m0[1, 1]))  # 1.0 0.0 0.0 1.0
