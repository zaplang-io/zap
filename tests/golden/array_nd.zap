# Golden fixture: N-D numeric arrays — shape + multi-axis indexing (#500, D-0164).
# A nested rectangular list literal builds a shaped (ndim > 1) array; `m[i, j]`
# indexes a scalar, a partial index `m[i]` is a row view that aliases the parent,
# and `.shape`/`.ndim`/`.size`/`len` expose the shape. Element-wise ops and the
# `sum` reduction run flat over the whole N-D shape.
def main():
    # 2-D construction (shape 2x3): shape tuple, ndim, size, first-axis len.
    auto m = array[i64]([[1, 2, 3], [4, 5, 6]])
    print("shape", m.shape[0], m.shape[1])     # 2 3
    print("ndim", m.ndim)                       # 2
    print("size", m.size)                       # 6
    print("len", len(m))                        # 2 (the first axis, NumPy len)
    print("elem", m[0, 0], m[0, 2], m[1, 1])    # 1 3 5
    print("sum", sum(m))                        # 1+2+3+4+5+6 -> 21

    # Partial index -> a rank-1 row view; a write through it aliases the parent.
    auto row = m[1]
    print("row", len(row), row[0], row[2])      # 3 4 6
    row[0] = 40
    print("alias", m[1, 0])                     # 40

    # Full-index store + augmented assignment to a scalar slot.
    m[0, 1] = 20
    m[1, 2] += 100
    print("set", m[0, 1], m[1, 2])              # 20 106

    # Element-wise on equal-shape 2-D arrays, scalar broadcast, mixed promotion.
    auto a = array[i64]([[1, 2], [3, 4]])
    auto b = array[i64]([[10, 20], [30, 40]])
    print("add", sum(a + b))                    # 11+22+33+44 -> 110
    print("mul", sum(a * 2))                    # 2+4+6+8 -> 20
    print("promote", sum(a + 0.5))              # 1.5+2.5+3.5+4.5 -> 12.0

    # 3-D construction + full multi-axis index.
    auto c = array[i64]([[[1, 2], [3, 4]], [[5, 6], [7, 8]]])
    print("cube", c.ndim, c.size, c[1, 0, 1], c[0, 1, 0])  # 3 8 6 3

    # A row of a 3-D array is a 2-D view; indexing it again reaches a scalar.
    auto plane = c[0]
    print("plane", plane.ndim, plane.shape[0], plane.shape[1], plane[1, 1])  # 2 2 2 4
