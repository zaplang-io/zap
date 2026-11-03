# claim: SET-1
# SPEC §4.6 (#608/#609, D16/D17): `set` is Stable; `set[T]()` builds an empty
# set (parallel to heap[T]()/channel[T]()), and `set(iterable)` /
# `set[T](iterable)` build a deduplicated set from any iterable — a list, a
# string (its codepoints), or another set. Iteration order is unspecified, so
# len and element sums pin the behavior order-independently.
def main():
    auto e = set[int64]()
    print("empty", len(e))
    auto a = set([1, 1, 2, 3, 3])
    auto sa = 0
    for x in a:
        sa = sa + x
    print("list", len(a), sa)
    auto b = set[int64]([5, 5, 6])
    print("typed", len(b), 6 in b)
    auto cs = set("aab")
    print("str", len(cs))
    auto d = set(a)
    print("set", len(d))
