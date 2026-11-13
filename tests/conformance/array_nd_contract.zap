# claim: ARR-2
# SPEC §15 (#500): N-D shape + multi-axis indexing. A nested rectangular literal
# builds a shaped array; `m[i, j]` names a scalar, a partial index `m[i]` is a
# rank-reduced row view that aliases the parent, `.shape`/`.ndim`/`.size` expose
# the shape, and element-wise ops / `sum` run over the whole shape. Unequal
# shapes (equal rank) raise a catchable ValueError before the kernel runs.
def main():
    auto m = array[i64]([[1, 2, 3], [4, 5, 6]])
    print("shape", m.ndim, m.shape[0], m.shape[1], m.size, len(m))
    print("get", m[0, 0], m[1, 2])
    print("sum", sum(m))
    m[0, 1] = 20
    m[1, 0] += 100
    print("set", m[0, 1], m[1, 0])
    auto row = m[1]
    print("row", row.ndim, len(row), row[2])
    row[0] = 70
    print("alias", m[1, 0])
    auto a = array[f64]([[1.0, 2.0], [3.0, 4.0]])
    print("ew", sum(a + a), sum(a * 2.0))
    auto c = array[i64]([[[1, 2]], [[3, 4]]])
    print("cube", c.ndim, c.size, c[1, 0, 0])
    try:
        auto bad = m + array[i64]([[1], [2]])
        print(sum(bad))
    except ValueError as e:
        print("shape error", e)
