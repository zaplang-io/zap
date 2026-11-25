# Golden fixture: variadic tuple[T] ordering, concatenation, slicing (#1475).
# Ordering is lexicographic (shorter-is-less on a common prefix); `+` concatenates
# into a fresh tuple; a slice is a fresh tuple[T].
def main():
    auto a = tuple([1, 2, 3])
    auto b = tuple([1, 2, 4])
    auto c = tuple([1, 2])

    # lexicographic ordering
    print(a < b)  # True   (3 < 4 at index 2)
    print(b < a)  # False
    print(c < a)  # True   (shorter prefix is less)
    print(a <= a)  # True
    print(a > c)  # True
    print(a >= b)  # False

    # concatenation
    auto cat = a + c
    print(cat)  # (1, 2, 3, 1, 2)
    print(len(cat))  # 5

    # slicing -> tuple[T]
    auto t = tuple([10, 20, 30, 40, 50])
    print(t[1:4])  # (20, 30, 40)
    print(t[:2])  # (10, 20)
    print(t[3:])  # (40, 50)
    print(t[::2])  # (10, 30, 50)
    print(t[::-1])  # (50, 40, 30, 20, 10)

    # str-element ordering
    print(tuple(["a", "b"]) < tuple(["a", "c"]))  # True
