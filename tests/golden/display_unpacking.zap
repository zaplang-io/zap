# Golden fixture: `*`/`**` unpacking in list/set/tuple/dict displays (#598). A
# `*expr` splices an iterable's elements into a list/set/tuple in place, and a
# `**expr` merges another dict's entries with last-wins semantics. Covers
# multiple splats, mixing with ordinary elements, a tuple splat (static arity),
# and a dict merge whose later entry overrides an earlier key. Sums and lengths
# are order-independent, so the output is deterministic for sets.
def main():
    list[int64] xs = [2, 3]
    list[int64] ys = [1, *xs, 4, *xs]
    auto sy = 0
    for y in ys:
        sy = sy + y
    print(sy)
    print(len(ys))

    set[int64] s = {*xs, 5, *[5, 6]}
    auto ss = 0
    for e in s:
        ss = ss + e
    print(ss)
    print(len(s))

    auto tup = (10, 20)
    auto t = (1, *tup, 2)
    print(t[0] + t[1] + t[2] + t[3])

    dict[str, int64] base = {"a": 1, "b": 2}
    dict[str, int64] m = {"a": 100, **base, "c": 3}
    print(m["a"])
    print(m["b"])
    print(m["c"])
    print(len(m))

    dict[str, int64] m2 = {**base, "a": 9}
    print(m2["a"])
