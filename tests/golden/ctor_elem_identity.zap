# Golden fixture: list/set/tuple(iterable) preserve bool/char element identity
# (#1513). Iterating a `list[bool]`/`list[char]` source through the general
# D-0048 protocol yielded the element's physical `i64` storage word, so the
# `X(iterable)` constructors re-derived an `int` element and the result rendered
# `1`/`0` (or the decimal codepoint) instead of `True`/`False` (or a quoted
# char). Narrowing the yielded word back to its logical `bool`/`char` at the
# protocol chokepoint fixes the constructors, comprehensions, and splats
# uniformly (a direct `[..]` literal always kept the identity). Char literals
# come from `chr()`/`char()`/string indexing — `'a'` is a `str` — so the char
# sources here are genuine `list[char]`s. Set/dict iteration order is unspecified
# (#1066), so the set cases use single-element reprs (deterministic) or `len`;
# the ordered list/tuple cases assert full reprs.
def main():
    # bool elements through list() / tuple() — the source is a list[bool]
    print(list([True, False, True]))  # [True, False, True]
    print(tuple([True, False]))  # (True, False)
    print(tuple([True]))  # (True,)  single-element trailing comma

    # char elements through list() / tuple() — genuine list[char] sources
    list[char] cs = [chr(97), chr(98), chr(99)]
    print(list(cs))  # ['a', 'b', 'c']
    print(tuple(cs))  # ('a', 'b', 'c')
    auto s = "xy"
    list[char] cs2 = [s[0], s[1]]
    print(tuple(cs2))  # ('x', 'y')
    print(tuple([char(109)]))  # ('m',)  single-element char tuple

    # str elements (Elem::Ptr) are unaffected but covered for completeness
    print(list(["a", "bb"]))  # ['a', 'bb']
    print(tuple(["p", "q"]))  # ('p', 'q')

    # set() — single-element reprs are deterministic; dedup by value via len
    print(set([True]))  # {True}
    print(set([chr(122)]))  # {'z'}
    print(len(set([True, False, True])))  # 2

    # empty constructors keep their Python-exact forms
    print(list([]))  # []
    print(tuple([]))  # ()

    # the same protocol backs comprehensions and splats, so they preserve it too
    print([b for b in [True, False]])  # [True, False]
    print([c for c in cs])  # ['a', 'b', 'c']
    auto flags = [True, False]
    print([*flags, True])  # [True, False, True]
    print([False, *flags])  # [False, True, False]
