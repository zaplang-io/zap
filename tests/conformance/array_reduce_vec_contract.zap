# claim: ARR-13
# SPEC §15 (#911): the reduction kernels (sum/min/max/mean/prod, whole-array and
# axis=) are vectorized with multiple independent accumulators plus a scalar
# tail; this is a pure performance change with NO effect on results. Integer
# reductions stay bit-identical at any size, and axis reductions preserve their
# per-element accumulation order for every dtype. This contract pins that an
# array large enough to cross the unroll boundary reduces to the same value the
# scalar reference computes.
def main():
    # Whole-array integer reductions over 24 elements (unrolled body + tail).
    auto a = arange[i64](1, 25)  # 1..24
    print("sum", a.sum())  # 300
    print("prod", a.min(), a.max())  # 1 24
    # int64 accumulation: a narrow-dtype sum that overflows its own width still
    # totals correctly.
    auto w = ones[i32](20) * 200000000
    print("wide", w.sum())  # 4000000000
    # Axis reductions over a large inner (contiguous j-loop) and a last axis.
    auto m = arange[i64](0, 40).reshape(2, 20)
    print("axis0", sum(m.sum(axis=0)))  # 780
    print("axis1", sum(m.sum(axis=1)))  # 780
    print("maxmin", sum(m.max(axis=0)), sum(m.min(axis=1)))  # 590 20
    # Float axis reduction stays bit-identical (per-lane order preserved).
    auto mf = array[f64]([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]])
    auto f0 = mf.sum(axis=0)
    print("float", str(f0[0]), str(f0[1]), str(f0[2]))  # 5.0 7.0 9.0
