# Golden fixture: list[T]() / dict[K, V]() empty constructors (#1004).
#
# The construction synonyms for the `[]` / `{}` empty literals (parity with
# `set[T]()`): they build the same empty collection and then behave like any
# list / dict. (A type-parameter element — `list[T]()` inside a generic body —
# is a documented follow-up; it needs the monomorphizer to substitute the
# constructor's type-argument subscript, which it does not do for `set[T]()`
# either.)
def main():
    auto a = list[int64]()
    print(len(a))  # 0
    a.append(10)
    a.append(20)
    print(len(a))  # 2
    print(a[0])  # 10
    print(a[1])  # 20

    auto d = dict[str, int64]()
    print(len(d))  # 0
    d["x"] = 1
    d["y"] = 2
    print(len(d))  # 2
    print(d["x"])  # 1
    print(d["y"])  # 2
