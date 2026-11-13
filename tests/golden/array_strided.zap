# Golden fixture: strided slice views (#502, D-0171). A stepped/reversed 1-D
# slice, an N-D axis-0 window, and a sliced axis inside a multi-axis index are
# all VIEWS that alias the parent buffer; arithmetic over a strided operand
# compacts it to a contiguous copy first (so the result is still correct).
def main():
    auto a = array[i64]([10, 20, 30, 40, 50, 60])

    # every other element (stride 2)
    auto ev = a[::2]  # [10, 30, 50]
    print("step2", ev.ndim, len(ev), ev[0], ev[1], ev[2], sum(ev))

    # reversed (negative stride)
    auto rv = a[::-1]  # [60, 50, 40, 30, 20, 10]
    print("rev", rv[0], rv[5], sum(rv))

    # stepped sub-range
    auto s = a[1:6:2]  # [20, 40, 60]
    print("sub", s[0], s[1], s[2], sum(s))

    # a write through a strided view aliases the parent
    auto w = a[::2]  # view over indices 0, 2, 4
    w[1] = 99  # writes a[2]
    print("alias", a[2], sum(a))  # 99, 10+20+99+40+50+60 = 279

    # arithmetic over a strided operand (compacted first)
    print("arith", sum(a[::2] + 1))  # (10+1)+(99+1)+(50+1) = 162

    # --- N-D strided views ---
    auto m = array[i64]([[1, 2, 3], [4, 5, 6], [7, 8, 9]])

    # N-D axis-0 window keeps rank
    auto top = m[0:2]  # [[1,2,3],[4,5,6]]
    print("nd", top.ndim, len(top), sum(top))  # 2 2 21

    # column view (sliced axis 0, integer axis 1)
    auto col = m[:, 1]  # [2, 5, 8]
    print("col", col.ndim, len(col), col[0], col[1], col[2], sum(col))

    # a row then a stepped slice
    auto r = m[2, ::2]  # [7, 9]
    print("mix", sum(r))  # 16

    # strided N-D operand in arithmetic
    print("ndarith", sum(m[:, 1] * 2))  # (2+5+8)*2 = 30
