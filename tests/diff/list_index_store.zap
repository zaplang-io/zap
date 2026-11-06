# diff: indexed STORES into list elements must match CPython's list semantics,
# including storing a value READ BACK from a list element — the shape that
# regressed in Zap (#727: `want[i] = flags[j]` on a list[bool] raised a bogus
# "indexed assignment expects `i64`, found `i64`"). CPython lists are untyped, so
# this is a pure semantics check across element types (int, bool, float, str)
# and index forms (literal, range-derived loop var, arithmetic `d - 1`, and an
# index that is itself an indexed load `a[b[i]]`). No divergence intended.
def main():
    # list[bool], auto (i64) index — the exact issue repro.
    list[bool] flags = []
    for i in range(4):
        flags.append(False)
    auto depth = 0
    flags[depth] = True
    print(flags)

    # jwt-style: store a bool read back from a list[bool], arithmetic index.
    list[bool] objects = []
    list[bool] want_key = []
    for i in range(4):
        objects.append(False)
        want_key.append(False)
    objects[3] = True
    auto d = 3
    want_key[d - 1] = objects[d]
    print(objects)
    print(want_key)

    # list[int]: literal, loop-var, read-back and nested-index (a[b[i]]) forms.
    list[int] xs = []
    for i in range(5):
        xs.append(i)
    xs[0] = 10
    for i in range(5):
        xs[i] = xs[i] + 1
    list[int] pos = [4, 2]
    xs[pos[0]] = xs[pos[1]]
    print(xs)

    # list[float]: a float read back from the list stored into another element.
    list[float] fs = []
    for i in range(3):
        fs.append(0.0)
    fs[0] = 1.5
    fs[2] = fs[0]
    print(fs)

    # list[str]: a str element read back and stored (pointer element, unchanged).
    list[str] ss = ["x", "y", "z"]
    ss[0] = ss[2]
    print(ss)
