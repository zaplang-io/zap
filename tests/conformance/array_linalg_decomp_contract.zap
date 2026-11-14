# claim: ARR-18
# SPEC §15 (#927): `import linalg` provides the LAPACK-backed matrix
# decompositions cholesky(a), qr(a) and svd(a) over numeric arrays, reduced form.
#   cholesky(a) -> L           (lower-triangular, a = L @ L.T; a SPD)
#   qr(a)       -> (Q, R)       (a (m x n) -> Q (m x k), R (k x n), k=min(m,n))
#   svd(a)      -> (U, S, Vt)   (a (m x n) -> U (m x k), S (k), Vt (k x n))
# Q/R and U/Vt are sign/gauge-ambiguous, so this contract pins the well-defined
# facts: cholesky's exact factor, singular values, result shapes, R's structural
# zero. A non-positive-definite cholesky raises a catchable ValueError. f64.
import linalg

def main():
    auto cl = linalg.cholesky(array[f64]([[4.0, 2.0], [2.0, 5.0]]))
    print("chol", str(cl[0, 0]), str(cl[0, 1]), str(cl[1, 0]), str(cl[1, 1]))  # 2.0 0.0 1.0 2.0

    auto q, r = linalg.qr(array[f64]([[1.0, 0.0], [0.0, 1.0], [0.0, 0.0]]))
    print("qr", q.shape[0], q.shape[1], r.shape[0], r.shape[1], str(r[1, 0]))  # 3 2 2 2 0.0

    auto u, s, vt = linalg.svd(array[f64]([[3.0, 0.0], [0.0, 2.0]]))
    print("svdshape", u.shape[0], u.shape[1], len(s), vt.shape[0], vt.shape[1])  # 2 2 2 2 2
    print("svds", str(s[0]), str(s[1]))  # 3.0 2.0

    try:
        auto bad = linalg.cholesky(array[f64]([[1.0, 2.0], [2.0, 1.0]]))
        print(str(bad[0, 0]))
    except ValueError:
        print("not-pd raises")
