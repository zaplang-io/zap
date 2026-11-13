# Golden fixture: element-wise broadcasting (#501, D-0165).
# NumPy rules: right-align shapes, stretch size-1 axes, extend the lower-rank
# operand with leading 1s. The result takes the larger rank and the larger of
# each aligned dim. Equal shapes keep the flat SIMD path; incompatible shapes
# raise a catchable ValueError.
def main():
    auto m = array[i64]([[1, 2, 3], [4, 5, 6]])  # (2, 3)

    # Row vector (3,) stretches across the rows (axis 0).
    auto r1 = m + array[i64]([10, 20, 30])
    print("rowb", r1[0, 0], r1[0, 2], r1[1, 1], sum(r1))  # 11 33 25 141

    # Column vector (2, 1) stretches across the columns (axis 1).
    auto r2 = m + array[i64]([[100], [200]])
    print("colb", r2[0, 0], r2[1, 2], sum(r2))  # 101 206 921

    # A (1,) operand rank-extends to (1, 1) and stretches both axes (scalar-like).
    auto r3 = m * array[i64]([5])
    print("oneb", r3[1, 0], sum(r3))  # 20 105

    # Both operands stretch: (1, 3) + (2, 1) -> (2, 3).
    auto r4 = array[i64]([[1, 2, 3]]) + array[i64]([[10], [20]])
    print("outer", r4.ndim, r4.shape[0], r4.shape[1], r4[0, 2], r4[1, 0], sum(r4))  # 2 2 3 13 21 102

    # Equal shapes: the flat (non-broadcast) kernel.
    print("equal", sum(m + m))  # 42

    # f64 row-vector broadcast.
    auto fm = array[f64]([[1.0, 2.0], [3.0, 4.0]])
    print("fbroad", sum(fm + array[f64]([10.0, 20.0])))  # 70.0

    # Incompatible shapes raise a catchable ValueError (axis 1: 3 vs 2).
    try:
        auto bad = m + array[i64]([1, 2])
        print(sum(bad))
    except ValueError:
        print("broadcast error")
