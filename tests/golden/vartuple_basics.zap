# Golden fixture: variadic tuple[T] Phase 1 (#1473) — the type, tuple(iterable)
# construction, indexing, iteration, len, and the Python-parens repr. Equality,
# hashing, ordering, concat and slicing are later phases (#1474/#1475).
def main():
    # construct from an iterable
    auto t = tuple([10, 20, 30])
    print(t)  # (10, 20, 30)
    print(len(t))  # 3
    print(t[0])  # 10
    print(t[2])  # 30
    print(t[-1])  # 30  (negative index, like a list)

    # iterate
    int64 s = 0
    for x in t:
        s = s + x
    print(s)  # 60

    # from range / str
    print(tuple(range(3)))  # (0, 1, 2)
    print(tuple("ab"))  # ('a', 'b')

    # single-element (trailing comma) and empty
    print(tuple([7]))  # (7,)
    tuple[int64] e = tuple[int64]()
    print(e)  # ()
    print(len(e))  # 0

    # typed construction + annotation round-trip
    tuple[int64] ti = tuple[int64]([1, 2])
    print(ti)  # (1, 2)
    tuple[int64] ann = tuple([5, 6])
    print(ann[1])  # 6
