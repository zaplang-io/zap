# claim: FN-9
# SPEC §7 (#597): `*expr` unpacks an iterable/list into positional arguments
# and `**expr` unpacks a `dict[str, V]` into keyword arguments, building on the
# variadic / `**kwargs` infrastructure. Dict access is by known key, so the
# expected output does not depend on dict iteration order.
def sum_all(int64... nums) -> int64:
    auto t = 0
    for n in nums:
        t = t + n
    return t


def tally(int64 base, int64 **counts) -> int64:
    return base + counts["a"] + counts["b"]


def main():
    auto xs = [1, 2, 3, 4]
    print(sum_all(*xs))
    print(sum_all(10, *xs))
    auto d = {"a": 7, "b": 3}
    print(tally(0, **d))
