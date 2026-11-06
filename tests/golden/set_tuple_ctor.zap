# Golden fixture: the typed empty set constructor with a tuple (non-name)
# element type (#703). Before the fix, `set[(int64, int64)]()` rejected with
# `ZAP6001 "set[...] expects an element type name"`, even though the set literal
# `{(1, 2)}` and `dict[(..), ..]` keys already accept tuples (D-0122). The
# constructor now accepts any valid element type expression; a tuple element
# routes through the synthesized structural `__hash__`/`__eq__` key path, exactly
# like the literal. Exercises `.add` (new + structural duplicate), `len`,
# membership (`in`, present and absent), nested tuples, and a (int, str) element.
def main():
    # (int64, int64) elements via the typed empty constructor.
    set[(int64, int64)] s = set[int64, int64]()
    print(len(s))  # 0
    s.add((1, 2))
    s.add((3, 4))
    s.add((1, 2))  # structural duplicate -> no-op
    print(len(s))  # 2
    print((1, 2) in s)  # True
    print((5, 6) in s)  # False

    # Nested-tuple elements: hashed/compared recursively.
    set[((int64, int64), int64)] sn = set[(int64, int64), int64]()
    sn.add(((1, 2), 3))
    sn.add(((1, 2), 3))  # structural duplicate
    sn.add(((1, 2), 4))
    print(len(sn))  # 2
    print(((1, 2), 3) in sn)  # True
    print(((9, 9), 3) in sn)  # False

    # (int64, str) elements: the str element compares by value.
    set[(int64, str)] ss = set[int64, str]()
    ss.add((1, "a"))
    ss.add((1, "a"))  # structural duplicate
    ss.add((2, "b"))
    print(len(ss))  # 2
    print((2, "b") in ss)  # True
    print((2, "c") in ss)  # False

    # Constructing from an iterable with a tuple element type also works.
    set[(int64, int64)] si = set[int64, int64]([(1, 2), (1, 2), (3, 4)])
    print(len(si))  # 2
