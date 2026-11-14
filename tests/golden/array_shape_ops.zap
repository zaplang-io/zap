# Golden fixture: shape ops reshape / transpose / .T / flatten / concatenate /
# stack (#511, D-0174). reshape & transpose/.T are views; flatten is a copy;
# concatenate/stack are fresh contiguous copies. Shape/dtype/axis faults raise a
# catchable ValueError.
def main():
    auto a = array[i64]([1, 2, 3, 4, 5, 6])

    # reshape (view): 1-D -> 2x3
    auto m = a.reshape(2, 3)
    print("reshape", m.ndim, m.shape[0], m.shape[1], m[0, 2], m[1, 0], sum(m))
    # one -1 axis is inferred
    auto r = a.reshape(3, -1)
    print("infer", r.shape[0], r.shape[1], r[2, 1])
    # reshape shares the buffer: a write through m is seen by a
    m[0, 0] = 99
    print("view", a[0])
    a[0] = 1  # restore

    # transpose / .T (view): 2x3 -> 3x2
    auto t = m.T
    print("T", t.ndim, t.shape[0], t.shape[1], t[0, 1], t[2, 0], sum(t))
    auto v1 = array[i64]([7, 8, 9])
    print("T1d", v1.T[0], v1.T[2], sum(v1.T))  # 1-D transpose is identity

    # flatten (copy): independent of the source
    auto f = m.flatten()
    print("flat", f.ndim, len(f), f[0], f[5], sum(f))
    f[0] = 100
    print("flatcopy", m[0, 0])

    # concatenate along axis 0 and axis 1
    auto p = array[i64]([[1, 2], [3, 4]])
    auto q = array[i64]([[5, 6], [7, 8]])
    auto c0 = concatenate([p, q])
    print("c0", c0.shape[0], c0.shape[1], c0[2, 0], c0[3, 1], sum(c0))
    auto c1 = concatenate([p, q], axis=1)
    print("c1", c1.shape[0], c1.shape[1], c1[0, 3], c1[1, 2], sum(c1))

    # stack (new axis)
    auto s0 = stack([p, q])
    print(
        "s0",
        s0.ndim,
        s0.shape[0],
        s0.shape[1],
        s0.shape[2],
        s0[0, 1, 0],
        s0[1, 0, 1],
        sum(s0),
    )
    auto s2 = stack([p, q], axis=2)
    print("s2", s2.shape[2], s2[0, 0, 0], s2[0, 0, 1], sum(s2))

    # faults raise a catchable ValueError
    try:
        auto bad = a.reshape(4, 2)
        print(sum(bad))
    except ValueError:
        print("reshape error")
    try:
        auto bad2 = concatenate([p, array[i64]([1, 2, 3])])
        print(sum(bad2))
    except ValueError:
        print("concat error")
