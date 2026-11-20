# Golden fixture (#1179): nullable `str` parameters honour `is None` / `== None`
# per D-0014. A reference-typed value (`str`, `bytes`, a collection) can hold
# `None`, so the null check is a real runtime test — distinguishing an omitted
# or explicit `None` argument from an empty or nonempty string — not a folded
# constant. `== None` / `!= None` behave identically to `is None` / `is not
# None` (SPEC §5). Ordinary value comparison of two strings (`==`) is unchanged.


def choose(str name = None) -> str:
    if name is None:
        return "local"
    return name


def describe(str name = None) -> str:
    # `== None` is the same null check as `is None` (SPEC §5).
    if name == None:
        return "none"
    if name == "":
        return "empty"
    return name


def main():
    # The #1179 repro: omitted / explicit None / empty / nonempty are distinct.
    print(choose())  # local
    print(choose(None))  # local
    print(choose(""))  # (empty line)
    print(choose("UTC"))  # UTC

    print(describe())  # none
    print(describe(None))  # none
    print(describe(""))  # empty
    print(describe("UTC"))  # UTC

    # Direct null checks on a nullable `str`.
    str present = "x"
    str absent = None
    print(str(present is None))  # False
    print(str(present is not None))  # True
    print(str(absent is None))  # True
    print(str(absent == None))  # True
    print(str(absent != None))  # False

    # `bytes` is nullable too (D-0014).
    bytes bz = None
    print(str(bz is None))  # True

    # A nullable collection distinguishes null from an allocated value.
    list[int64] xs = None
    list[int64] ys = [1, 2]
    print(str(xs is None))  # True
    print(str(ys is None))  # False

    # Ordinary value comparison of two distinct, equal strings is unchanged.
    str a = "hi"
    str b = "hi"
    print(str(a == b))  # True

    print("done")
