# Golden fixture: starred call arguments `f(*args, **kwargs)` (#597). A `*expr`
# unpacks a list/tuple into positional arguments and a `**expr` unpacks a
# `dict[str, V]` into keyword arguments; both build on the existing variadic /
# `**kwargs` infrastructure. Covers: spreading a list into a variadic, mixing a
# leading positional with a `*` spread, expanding a statically-sized tuple into
# fixed parameters, spreading a dict into a `**kwargs` collector, and the
# forwarding idiom `inner(*args, **kwargs)`. Sums are order-independent.
def sum_all(int64... nums) -> int64:
    auto t = 0
    for n in nums:
        t = t + n
    return t


def dist(int64 x, int64 y) -> int64:
    return x * x + y * y


def tally(int64 base, int64 **counts) -> int64:
    auto total = base
    for k in counts.keys():
        total = total + counts[k]
    return total


def forward(int64... args, int64 **opts) -> int64:
    return sum_all(*args) + tally(0, **opts)


def main():
    auto xs = [1, 2, 3, 4]
    print(sum_all(*xs))
    print(sum_all(10, *xs))
    auto p = (3, 4)
    print(dist(*p))
    auto d = {"k": 7, "m": 3}
    print(tally(100, **d))
    print(forward(1, 2, 3, p=10, q=20))
