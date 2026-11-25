# Golden fixture: map() / filter() built-ins (#1390). Both materialize to a
# `list` (the `reversed` model), so they compose with `for`, `list()`, and
# `sum()`. `map(func, *lists)` applies `func` across one or more lists, stopping
# at the shortest, into a `list[R]` (R = `func`'s return type); `filter(pred,
# list)` keeps the elements for which `pred` is truthy, preserving the element
# type. Named-function callees here; closures / method values and strictly-lazy
# adapters are documented follow-ups.

def square(int64 x) -> int64:
    return x * x

def add(int64 a, int64 b) -> int64:
    return a + b

def is_even(int64 x) -> bool:
    return x % 2 == 0

def main():
    # --- map over one list ---
    list[int64] ys = map(square, [1, 2, 3, 4])
    print(ys)  # [1, 4, 9, 16]
    print(len(ys))  # 4
    assert len(ys) == 4
    assert ys[3] == 16

    # --- map over two lists, stopping at the shortest ---
    list[int64] zs = map(add, [1, 2, 3], [10, 20, 30, 40])
    print(zs)  # [11, 22, 33]
    print(len(zs))  # 3
    assert len(zs) == 3

    # --- filter with a predicate (element type preserved) ---
    list[int64] evens = filter(is_even, [1, 2, 3, 4, 5, 6])
    print(evens)  # [2, 4, 6]
    assert len(evens) == 3
    assert evens[0] == 2

    # --- composition: sum(map(...)) and for over filter(...) ---
    print(sum(map(square, [1, 2, 3])))  # 14
    assert sum(map(square, [1, 2, 3])) == 14
    int64 total = 0
    for e in filter(is_even, [10, 11, 12, 13]):
        total = total + e
    print(total)  # 22
    assert total == 22

    # --- nested: filter the mapped values ---
    list[int64] big_evens = filter(is_even, map(square, [1, 2, 3, 4]))
    print(big_evens)  # [4, 16]
    assert len(big_evens) == 2

    # --- empty input ---
    list[int64] none = []
    list[int64] em = map(square, none)
    list[int64] ef = filter(is_even, none)
    print(len(em))  # 0
    print(len(ef))  # 0
    assert len(em) == 0
    assert len(ef) == 0
