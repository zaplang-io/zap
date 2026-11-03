# diff: match
# `**kwargs` collector parameters (#601) and starred call arguments
# `f(*args, **kwargs)` (#597) share CPython's semantics over this subset, so
# stdout must agree. Values are summed (order-independent) and dict access is by
# known key, so neither print depends on dict iteration order. The translator
# turns `int64... args` into `*args` and `int64 **opts` into `**opts`; the
# `*`/`**` call arguments are already valid Python and pass through unchanged.
def sum_all(int64... nums) -> int64:
    auto t = 0
    for n in nums:
        t = t + n
    return t


def tally(int64 base, int64 **counts) -> int64:
    return base + counts["a"] + counts["b"]


def forward(int64... args, int64 **opts) -> int64:
    return sum_all(*args) + tally(0, **opts)


def main():
    auto xs = [1, 2, 3, 4]
    print(sum_all(*xs))
    print(sum_all(10, *xs))
    auto d = {"a": 7, "b": 3}
    print(tally(100, **d))
    print(forward(1, 2, 3, a=10, b=20))
