# Golden fixture: array reductions sum/min/max/mean/prod with an optional
# axis= (#507, D-0170). Full reductions keep the dtype (mean is a float); an
# axis= collapses one axis to a rank-(n-1) array. Empty sum/prod/mean are
# defined (0/1/nan); empty min/max and an out-of-range axis raise ValueError.
def main():
    # 1-D full reductions (integer): dtype preserved, mean is a float.
    auto a = array[i64]([3, 1, 4, 1, 5, 9, 2, 6])
    print(a.sum())  # 31
    print(a.min())  # 1
    print(a.max())  # 9
    print(a.prod())  # 6480
    print(str(a.mean()))  # 3.875

    # 1-D full reductions (f64).
    auto g = array[f64]([2.0, 4.0, 6.0])
    print(str(g.sum()), str(g.min()), str(g.max()))  # 12.0 2.0 6.0
    print(str(g.prod()), str(g.mean()))  # 48.0 4.0

    # Axis reductions on a (2, 3) matrix -> rank-1 results.
    auto m = array[i64]([[1, 2, 3], [4, 5, 6]])
    auto c0 = m.sum(axis=0)
    print("c0", c0.ndim, len(c0), c0[0], c0[1], c0[2])  # c0 1 3 5 7 9
    auto c1 = m.sum(axis=1)
    print("c1", len(c1), c1[0], c1[1])  # c1 2 6 15
    print("mx", sum(m.max(axis=0)))  # mx 15
    print("mn", sum(m.min(axis=1)))  # mn 5
    print("pr", sum(m.prod(axis=0)))  # pr 32
    auto me = m.mean(axis=0)
    print("me", str(me[0]), str(me[1]), str(me[2]))  # me 2.5 3.5 4.5

    # Negative axis: axis=-1 == axis=1.
    print("neg", sum(m.sum(axis=-1)))  # neg 21

    # Empty array: sum/prod/mean defined; min/max raise ValueError.
    auto e = array[i64]([])
    print("empty", e.sum(), e.prod(), str(e.mean()))  # empty 0 1 nan
    try:
        print(e.min())
    except ValueError:
        print("min raises")

    # Out-of-range axis raises ValueError.
    try:
        print(sum(m.sum(axis=5)))
    except ValueError:
        print("axis raises")
