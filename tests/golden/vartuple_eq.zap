# Golden fixture: variadic tuple[T] equality (#1474, Phase 2 part 1). Element-
# wise `==` / `!=` over the frozen buffer: equal length + equal elements. Hashing
# / dict-set-key use is the remaining part of #1474 (needs the key-representation
# change). Ordering/concat/slicing are #1475.
def main():
    auto a = tuple([1, 2, 3])
    auto b = tuple([1, 2, 3])
    auto c = tuple([1, 2])
    auto d = tuple([1, 2, 4])
    print(a == b)  # True
    print(a == c)  # False  (different length)
    print(a == d)  # False  (different element)
    print(a != b)  # False
    print(a != c)  # True

    # str elements (compared by value, not pointer identity)
    auto s1 = tuple(["x", "y"])
    auto s2 = tuple(["x", "y"])
    auto s3 = tuple(["x", "z"])
    print(s1 == s2)  # True
    print(s1 == s3)  # False

    # two independently-built empty tuples are equal
    print(tuple[int64]() == tuple[int64]())  # True
