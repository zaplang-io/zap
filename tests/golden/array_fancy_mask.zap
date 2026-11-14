# Golden fixture: boolean-mask & fancy (integer-array) indexing + where (#510,
# D-0175). Element-wise comparisons and `& | ~` build masks; `a[mask]` /
# `a[idx]` gather copies; `a[mask]=`/`a[idx]=` scatter; where(cond,x,y) selects;
# where(cond) is nonzero; multi-advanced + mixed basic/advanced follow NumPy's
# transpose rule; an array in a boolean context and an out-of-bounds fancy index
# raise a catchable ValueError / IndexError.
def main():
    auto a = array[i64]([1, 2, 3, 4, 5, 6])

    # comparisons -> boolean masks (sum of a bool array counts the True cells)
    auto m = a > 3
    print("mask", sum(m))
    auto b = array[i64]([6, 5, 4, 3, 2, 1])
    print("eq", sum(a == b))
    print("and", sum((a > 2) & (a < 5)))
    print("or", sum((a < 2) | (a > 5)))
    print("not", sum(~m))

    # boolean-mask select (a 1-D copy of the True elements)
    auto sel = a[a > 3]
    print("sel", len(sel), sel[0], sel[2], sum(sel))

    # full-shape 2-D mask -> 1-D; leading-axis mask keeps the trailing axis
    auto g = array[i64]([[1, 2], [3, 4]])
    auto gs = g[g > 2]
    print("2dmask", len(gs), gs[0], gs[1], sum(gs))
    auto rowsel = array[i64]([1, 0]) > 0
    auto gr = g[rowsel]
    print("leadmask", gr.ndim, gr.shape[0], gr.shape[1], sum(gr))

    # masked assignment: scalar clip, then array scatter
    auto c = array[i64]([-2, -1, 0, 1, 2])
    c[c < 0] = 0
    print("clip", c[0], c[1], c[4], sum(c))
    auto c2 = array[i64]([1, 2, 3, 4])
    c2[c2 > 2] = array[i64]([7, 8])
    print("maskset", c2[2], c2[3], sum(c2))

    # fancy integer gather (1-D, negative wrap, N-D on axis 0)
    auto d = array[i64]([10, 20, 30, 40])
    auto gg = d[array[i64]([0, 2, 2, 3])]
    print("gather", len(gg), gg[1], sum(gg))
    auto neg = d[array[i64]([-1, -2])]
    print("neg", neg[0], neg[1])
    auto e = array[i64]([[1, 2], [3, 4], [5, 6]])
    auto er = e[array[i64]([2, 0])]
    print("ndgather", er.shape[0], er.shape[1], er[0, 0], er[1, 1], sum(er))

    # fancy scatter
    auto f = array[i64]([1, 2, 3, 4])
    f[array[i64]([0, 2])] = 9
    print("scatter", f[0], f[2], sum(f))

    # where(cond, x, y): scalar fill and two arrays
    auto w = array[i64]([-2, 3, -4, 5])
    auto wr = where(w > 0, w, 0)
    print("where", wr[0], wr[1], sum(wr))
    auto x = array[i64]([10, 20, 30])
    auto y = array[i64]([1, 2, 3])
    auto wr2 = where(x > 15, x, y)
    print("where2", wr2[0], wr2[1], sum(wr2))

    # multiple advanced indices + mixed basic/advanced (transpose rule)
    auto mm = array[i64]([[1, 2, 3], [4, 5, 6], [7, 8, 9]])
    auto ii = array[i64]([0, 2])
    auto jj = array[i64]([1, 0])
    auto mf = mm[ii, jj]
    print("fancy2", len(mf), mf[0], mf[1], sum(mf))
    auto ms = mm[1:3, jj]
    print("mixed", ms.shape[0], ms.shape[1], ms[0, 0], ms[1, 1], sum(ms))
    auto mfa = mm[ii, :]
    print("full", mfa.shape[0], mfa.shape[1], mfa[1, 2], sum(mfa))

    # array truthiness is ambiguous; an out-of-bounds fancy index raises
    try:
        if a > 3:
            print("bad")
    except ValueError:
        print("ambig")
    try:
        auto oob = d[array[i64]([0, 9])]
        print("noraise", oob[0])
    except IndexError:
        print("oob")

    # where(cond) -> a tuple of index arrays (one per axis)
    auto nz = where(a > 4)
    print("nonzero", len(nz[0]), nz[0][0], nz[0][1])
