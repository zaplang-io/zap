# Golden fixture: collection constructors with a type-parameter element (#1013).
#
# `list[T]()` / `dict[K,V]()` / `set[T]()` inside a generic body — where no `[]`
# / `{}` literal can name the element type — must monomorphise to the concrete
# element type. Exercised across several instantiations (int64 + str for the
# list) to confirm each specialization builds the right collection.
def collect_list[T](T x, T y) -> int64:
    auto xs = list[T]()
    xs.append(x)
    xs.append(y)
    return len(xs)

def collect_dict[K, V](K k, V v) -> int64:
    auto d = dict[K, V]()
    d[k] = v
    return len(d)

def collect_set[T](T x, T y) -> int64:
    auto s = set[T]()
    s.add(x)
    s.add(y)
    return len(s)

def main():
    print(collect_list[int64](1, 2))  # 2
    print(collect_list[str]("a", "b"))  # 2
    print(collect_dict[str, int64]("k", 9))  # 1
    print(collect_set[int64](3, 3))  # 1 (dedup)
    print(collect_set[int64](3, 4))  # 2
