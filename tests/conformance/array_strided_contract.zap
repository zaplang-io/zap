# claim: ARR-9
# SPEC §15 (#502): strided slice views. `a[::2]` / `a[::-1]` (1-D stepped /
# reversed), `m[lo:hi]` (N-D axis-0 window, rank-preserving), and a sliced axis
# inside a multi-axis index (`m[:, j]`) are all VIEWS that alias the parent
# buffer; arithmetic over a strided operand compacts it to contiguous first.
def main():
    auto a = array[i64]([1, 2, 3, 4, 5, 6])
    print("step", sum(a[::2]), sum(a[1::2]))
    auto rv = a[::-1]
    print("rev", rv[0], sum(rv))
    auto w = a[::2]
    w[0] = 10
    print("alias", a[0], sum(a))
    auto m = array[i64]([[1, 2, 3], [4, 5, 6]])
    print("axis0", m[0:1].ndim, sum(m[0:1]))
    print("col", sum(m[:, 2]))
    print("ndarith", sum(m[:, 0] + 10))
