# Golden fixture: vectorized reduction kernels (#911). The reducers run a
# multi-accumulator unrolled body (eight independent lanes) over the contiguous
# buffer, horizontally combine, then a scalar remainder. These cases use arrays
# large enough to cross that unroll boundary (>= 8 elements, with a non-empty
# tail) so BOTH the vectorized body and the scalar tail run, and the results
# must match the scalar reference exactly: integer reductions are bit-identical,
# and the float cases here use reassociation-exact values (sums/products that
# are exact in IEEE-754 regardless of accumulation order), so they too are
# stable. Axis reductions keep their per-element accumulation order, so they are
# bit-identical for every dtype.
def main():
    # --- whole-array, 20 elements (16-wide unrolled body + 4-element tail) ---
    # Nineteen 1s with a single 21 at index 9: the extreme lands in a non-first
    # accumulator lane, exercising the horizontal min/max combine.
    auto a = array[i64]([1, 1, 1, 1, 1, 1, 1, 1, 1, 21, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1])
    print("i64", a.sum(), a.min(), a.max(), a.prod(), str(a.mean()))  # 40 1 21 21 2.0

    auto g = array[f64]([1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 21.0,
                         1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0])
    print("f64", str(g.sum()), str(g.min()), str(g.max()), str(g.prod()), str(g.mean()))

    auto h = array[f32]([1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 21.0,
                         1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0])
    print("f32", str(h.sum()), str(h.max()), str(h.mean()))  # 40.0 21.0 2.0

    # Whole-array integer sum accumulates in int64, so a narrow-dtype sum that
    # overflows its own width still totals correctly (twenty * 2e8 = 4e9).
    auto w = ones[i32](20) * 200000000
    print("i32wide", w.sum())  # 4000000000

    # --- larger arange sums cross many unrolled iterations ---
    auto r = arange[i64](0, 40)  # 0..39
    print("arange", r.sum(), r.min(), r.max())  # 780 0 39

    # A strided view compacts to a dense buffer first, then the unrolled kernel.
    auto s2 = r[::2]  # 0, 2, ..., 38  (20 elements)
    print("strided", s2.sum(), s2.min(), s2.max())  # 380 0 38

    # --- axis reductions on a (3, 4) matrix ---
    # axis=1 collapses the last (contiguous) axis -> the inner==1 helper path;
    # axis=0 collapses a leading axis -> the row-wise loop-interchange path.
    auto m = array[i64]([[1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12]])
    auto c0 = m.sum(axis=0)
    print("sum0", c0[0], c0[1], c0[2], c0[3])  # 15 18 21 24
    auto c1 = m.sum(axis=1)
    print("sum1", c1[0], c1[1], c1[2])  # 10 26 42
    print("max0", sum(m.max(axis=0)))  # 9+10+11+12 = 42
    print("min1", sum(m.min(axis=1)))  # 1+5+9 = 15
    print("prod0", sum(m.prod(axis=0)))  # 45+120+231+384 = 780
    auto me = m.mean(axis=1)
    print("mean1", str(me[0]), str(me[1]), str(me[2]))  # 2.5 6.5 10.5

    # --- axis reduction with a large inner (the vectorized j-loop) ---
    auto big = arange[i64](0, 40).reshape(2, 20)
    auto b0 = big.sum(axis=0)  # [20, 22, ..., 58]
    print("big0", len(b0), b0[0], b0[19], sum(b0))  # 20 20 58 780
    print("big1", sum(big.sum(axis=1)))  # 190 + 590 = 780

    # --- float axis reductions stay bit-identical (per-lane order preserved) ---
    auto mf = array[f64]([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]])
    auto f0 = mf.sum(axis=0)
    print("f0", str(f0[0]), str(f0[1]), str(f0[2]))  # 5.0 7.0 9.0
    auto fm = mf.mean(axis=0)
    print("fm", str(fm[0]), str(fm[1]), str(fm[2]))  # 2.5 3.5 4.5
