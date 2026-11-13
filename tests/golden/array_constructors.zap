# Golden fixture: array constructors zeros / ones / full / arange / linspace
# (#504, D-0167). Shape is an int (1-D) or a tuple literal (N-D); arange/linspace
# build runtime-length 1-D arrays.
def main():
    # zeros / ones over 1-D and N-D shapes.
    print("zeros", sum(zeros[f64](4)), sum(zeros[i64]((2, 3))))  # 0.0 0
    print("ones", sum(ones[i64](5)), sum(ones[f64]((2, 2))))  # 5 4.0
    auto z = ones[i64]((2, 3))
    print("ones_shape", z.ndim, z.shape[0], z.shape[1], z[1, 2])  # 2 2 3 1

    # full: a shaped array of one value.
    auto f = full[i64]((2, 2), 7)
    print("full", sum(f), f[0, 0])  # 28 7
    print("full_f", sum(full[f64](3, 1.5)))  # 4.5

    # arange: start, start+step, ... stopping before stop.
    auto r = arange[i64](0, 10, 2)
    print("arange", len(r), r[0], r[4], sum(r))  # 5 0 8 20
    print("arange_dflt", sum(arange[i64](1, 5)))  # 10
    auto rf = arange[f64](0.0, 1.0, 0.25)
    print("arange_f", len(rf), rf[0], rf[3])  # 4 0.0 0.75
    print("arange_empty", len(arange[i64](5, 5)))  # 0

    # linspace: num evenly spaced samples, both ends inclusive.
    auto ls = linspace[f64](0.0, 1.0, 5)
    print("linspace", len(ls), ls[0], ls[2], ls[4])  # 5 0.0 0.5 1.0
    print("linspace_one", len(linspace[f64](2.0, 9.0, 1)))  # 1

    # A zero arange step raises a catchable ValueError.
    try:
        auto bad = arange[i64](0, 10, 0)
        print(len(bad))
    except ValueError:
        print("step error")
