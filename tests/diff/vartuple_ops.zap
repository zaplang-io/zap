# diff: match
# tuple[T] ordering (< <= > >=), concatenation (+), and slicing match CPython's
# tuple (#1475): lexicographic order, concat into a fresh tuple, slice to a
# tuple. Only tuple(iterable) is used (the typed/empty forms are golden-only).
def main():
    auto a = tuple([1, 2, 3])
    auto b = tuple([1, 2, 4])
    auto c = tuple([1, 2])
    print(a < b)
    print(b < a)
    print(c < a)
    print(a <= a)
    print(a > c)
    print(a >= b)
    print(a + c)
    print(len(a + c))
    auto t = tuple([10, 20, 30, 40, 50])
    print(t[1:4])
    print(t[:2])
    print(t[3:])
    print(t[::2])
    print(t[::-1])
    print(tuple(["a", "b"]) < tuple(["a", "c"]))
