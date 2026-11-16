# Golden fixture: list[T]() / dict[K, V]() empty constructors (#1004).
#
# The generic-context synonyms for the `[]` / `{}` empty literals (parity with
# `set[T]()`): they build the same empty collection and then behave like any
# list/dict. `collect[T]` exercises the real motivation — an element type that
# is a type parameter, where no `[]` literal can name the type — across two
# monomorphizations.
def collect[T](T x, T y) -> int64:
    auto xs = list[T]()
    xs.append(x)
    xs.append(y)
    return len(xs)

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

    print(collect[int64](1, 2))  # 2
    print(collect[str]("a", "b"))  # 2
