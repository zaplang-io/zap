# claim: ARR-3
# SPEC §15 (#501): element-wise broadcasting. Shapes right-align; a size-1 (or
# missing, lower-rank) axis stretches; the result takes the larger rank and the
# larger aligned dim. Incompatible shapes raise a catchable ValueError before
# the kernel runs.
def main():
    auto m = array[i64]([[1, 2, 3], [4, 5, 6]])
    print("row", sum(m + array[i64]([10, 20, 30])))
    print("col", sum(m + array[i64]([[100], [200]])))
    print("outer", sum(array[i64]([[1, 2, 3]]) + array[i64]([[10], [20]])))
    print("equal", sum(m + m))
    try:
        auto bad = m + array[i64]([1, 2])
        print(sum(bad))
    except ValueError as e:
        print("error", e)
