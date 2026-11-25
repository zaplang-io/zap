# diff: match
# Exhaustive element-kind coverage for variadic tuple[T] (#1476): float, bool,
# int and str elements across repr, ==, +, len, index, and slicing — all
# oracle-checked against CPython's tuple. (f64 ordering is intentionally not
# exercised; hashing / typed-ctor forms live in the golden suite.)
def main():
    # float elements
    print(tuple([1.5, 2.5, 3.5]))
    print(tuple([1.5, 2.5]) == tuple([1.5, 2.5]))
    print(tuple([1.5, 2.5]) == tuple([1.5, 9.5]))
    print(tuple([1.5]) + tuple([2.5, 3.5]))
    print(len(tuple([1.5, 2.5, 3.5])))
    print(tuple([1.5, 2.5, 3.5])[1])

    # bool elements
    print(tuple([True, False, True]))
    print(tuple([True, False]) == tuple([True, False]))

    # int ops: slicing combinations
    print(tuple(range(5))[1:4])
    print(tuple(range(5))[::-1])
    print(tuple(range(5))[::2])
    print(tuple(range(5)) < tuple([0, 1, 2, 9]))

    # str elements
    print(tuple(["a", "b"]) + tuple(["c"]))
    print(tuple(["a", "b", "c"])[0])
    print(tuple(["a", "b"]) < tuple(["a", "c"]))
