# claim: ARR-8
# SPEC §15 (#507): array reductions sum/min/max/mean/prod as methods, each with
# an optional axis=. Full reductions keep the dtype (mean is a float); axis=k
# collapses one axis to a rank-(n-1) array. Empty sum/prod/mean are 0/1/nan;
# empty min/max and an out-of-range axis raise a catchable ValueError.
def main():
    auto a = array[i64]([4, 2, 8, 1])
    print("full", a.sum(), a.min(), a.max(), a.prod(), str(a.mean()))
    auto m = array[i64]([[1, 2], [3, 4]])
    print("axis0", sum(m.sum(axis=0)), sum(m.max(axis=0)))
    print("axis1", sum(m.sum(axis=1)), sum(m.min(axis=1)))
    auto me = m.mean(axis=0)
    print("mean", str(me[0]), str(me[1]))
    auto e = array[i64]([])
    print("empty", e.sum(), e.prod())
    try:
        print(e.max())
    except ValueError:
        print("empty raises")
    try:
        print(sum(m.sum(axis=9)))
    except ValueError:
        print("axis raises")
