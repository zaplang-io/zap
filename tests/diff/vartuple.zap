# diff: match
# A variadic tuple[T] built by tuple(iterable) matches CPython's tuple (#1473):
# the `(…)` / `(x,)` repr, indexing (incl. negative), len, and iteration. The
# typed/empty `tuple[T](…)` forms are Zap-only subscript syntax (not valid
# Python), so they live in the golden suite; equality/hash/order/concat come in
# later phases.
def main():
    auto t = tuple([10, 20, 30])
    print(t)
    print(len(t))
    print(t[0])
    print(t[-1])
    int64 s = 0
    for x in t:
        s = s + x
    print(s)
    print(tuple(range(3)))
    print(tuple("ab"))
    print(tuple([7]))
    print(tuple([]))
