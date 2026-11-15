# Golden fixture: linalg decompositions cholesky / qr / svd (#927), LAPACK-backed.
# qr's Q/R and svd's U/Vt are sign/gauge-ambiguous (a reflection/sign flip is a
# valid factorization), so the deterministic, provider-portable facts are pinned
# here: cholesky's exact lower factor, singular values (well-defined), result
# shapes, and R's structural zero. Numeric reconstruction (Q@R==A, U·S·Vtᵀ==A) is
# verified by the runtime's own host checks. (Needs a BLAS/LAPACK provider.)
import linalg

def main():
    # cholesky: exact lower-triangular L with A = L @ L.T, for a clean SPD matrix.
    auto cl = linalg.cholesky(array[f64]([[4.0, 2.0], [2.0, 5.0]]))
    print("chol", str(cl[0, 0]), str(cl[0, 1]), str(cl[1, 0]), str(cl[1, 1]))  # 2.0 0.0 1.0 2.0

    # qr (reduced): Q (m x k), R (k x n) upper-triangular, k=min(m,n).
    auto q, r = linalg.qr(array[f64]([[0.0, 1.0], [1.0, 0.0]]))
    print("qrshape", q.ndim, q.shape[0], q.shape[1], r.shape[0], r.shape[1])  # 2 2 2 2 2
    print("qrzero", str(r[1, 0]))  # 0.0 (R is upper-triangular)

    # non-square qr: a (3 x 2) -> Q (3 x 2), R (2 x 2).
    auto q2, r2 = linalg.qr(array[f64]([[1.0, 0.0], [0.0, 1.0], [0.0, 0.0]]))
    print("qr32", q2.shape[0], q2.shape[1], r2.shape[0], r2.shape[1])  # 3 2 2 2

    # svd (reduced): U (m x k), S (k), Vt (k x n). S is well-defined; for a
    # diagonal matrix it is the sorted magnitudes.
    auto u, s, vt = linalg.svd(array[f64]([[3.0, 0.0], [0.0, 2.0]]))
    print("svdshape", u.shape[0], u.shape[1], len(s), vt.shape[0], vt.shape[1])  # 2 2 2 2 2
    print("svds", str(s[0]), str(s[1]), str(sum(s)))  # 3.0 2.0 5.0

    # non-positive-definite cholesky raises a catchable ValueError.
    try:
        auto bad = linalg.cholesky(array[f64]([[1.0, 2.0], [2.0, 1.0]]))
        print(str(bad[0, 0]))
    except ValueError:
        print("not-pd raises")
