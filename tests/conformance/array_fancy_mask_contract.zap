# claim: ARR-12
# SPEC §15 (#510, D-0175): boolean-mask & fancy (integer-array) indexing + where.
# `a[mask]` / `a[idx]` gather a fresh copy; `a[mask]=` / `a[idx]=` scatter in
# place; `where(cond, x, y)` selects element-wise with broadcasting; `where(cond)`
# is nonzero (a tuple of index arrays); multiple advanced indices + mixed
# basic/advanced follow NumPy. An array in a boolean context raises a catchable
# ValueError.
def main():
    auto a = array[i64]([1, 2, 3, 4, 5, 6])
    print("sel", sum(a[a > 3]))
    auto c = array[i64]([-1, 2, -3, 4])
    c[c < 0] = 0
    print("clip", sum(c))
    auto d = array[i64]([10, 20, 30, 40])
    print("gather", sum(d[array[i64]([0, 3, 3])]))
    print("where", sum(where(a > 3, a, 0)))
    auto nz = where(a > 4)
    print("nonzero", len(nz[0]))
    auto mm = array[i64]([[1, 2], [3, 4], [5, 6]])
    print("fancy2", sum(mm[array[i64]([0, 2]), array[i64]([1, 0])]))
    try:
        if a > 3:
            print("bad")
    except ValueError:
        print("err")
