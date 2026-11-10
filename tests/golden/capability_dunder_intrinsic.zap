# Golden: an explicit capability dunder (`__cmp__`/`__eq__`/`__hash__`, D-0139)
# called by name on a value of a type parameter bounded on a capability now
# lowers when the parameter is monomorphized to an INTRINSIC type — scalars,
# `char`, a tier-1 enum, `float`, `str`, `bytes` — emitting the same primitive the
# operator surface and the dict/set key path use (I#820). Previously this aborted
# at lowering (`ZAP6001: method call on a non-struct value`); the idiomatic
# operator form (`<`, `==`) already worked, so the two surfaces now agree.
enum Color:
    Red
    Green
    Blue


def cmp3[T: Comparable](T a, T b) -> int64:
    return a.__cmp__(b)


def eq[T: Equatable](T a, T b) -> bool:
    return a.__eq__(b)


def hsh[T: Hashable](T a) -> int64:
    return a.__hash__()


def main():
    # __cmp__ three-way (-1 / 0 / +1) on an int.
    print(cmp3[int64](1, 2))  # -1
    print(cmp3[int64](2, 2))  # 0
    print(cmp3[int64](5, 2))  # 1

    # __cmp__ on a float and on str/bytes (lexicographic).
    print(cmp3[float64](1.0, 2.0))  # -1
    print(cmp3[str]("a", "b"))  # -1
    print(cmp3[str]("b", "a"))  # 1

    # __cmp__ on a tier-1 enum (by backing value: Red=0 < Green=1).
    print(cmp3[Color](Color.Red, Color.Green))  # -1
    print(cmp3[Color](Color.Blue, Color.Blue))  # 0

    # __eq__ on int, char, bytes.
    print(eq[int64](5, 5))  # True
    print(eq[int64](5, 6))  # False
    str s = "aba"
    print(eq[char](s[0], s[2]))  # 'a' == 'a' -> True
    print(eq[char](s[0], s[1]))  # 'a' == 'b' -> False
    print(eq[bytes](b"x", b"x"))  # True

    # __hash__ is stable: equal values hash equal (the hash word itself is
    # unspecified, so compare two hashes rather than print one).
    print(hsh[int64](42) == hsh[int64](42))  # True
    print(hsh[str]("k") == hsh[str]("k"))  # True
    print(hsh[Color](Color.Green) == hsh[Color](Color.Green))  # True

    # __cmp__ agrees with the operator surface for the same operands.
    print((cmp3[int64](3, 7) < 0) == (3 < 7))  # True
