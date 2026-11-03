# Golden fixture: set constructors (#609, D17) — `set[T]()` empty construction,
# `set(iterable)` / `set[T](iterable)` dedup from a list, a string (codepoints),
# and another set. Iteration order is unspecified, so results are reduced
# order-independently (len and element sums).
def main():
    # set[T]() — empty, element type tracked for .add / len.
    auto e = set[int64]()
    print(len(e))  # 0
    e.add(7)
    e.add(7)
    print(len(e))  # 1

    # set(iterable) over a list, with duplicates removed.
    auto a = set([1, 1, 2, 3, 3, 3])
    print(len(a))  # 3
    auto sa = 0
    for x in a:
        sa = sa + x
    print(sa)  # 6

    # set[T](iterable) — element type stated explicitly.
    auto b = set[int64]([5, 5, 6])
    print(len(b))  # 2
    auto sb = 0
    for y in b:
        sb = sb + y
    print(sb)  # 11

    # set(str) — the unique codepoints of the string.
    auto cs = set("aab")
    print(len(cs))  # 2

    # set(set) — iterating another set is a shallow copy with dedup.
    auto d = set(a)
    print(len(d))  # 3
