# diff: match
# Dict lookup and membership that are order-independent: `len`, indexed reads,
# and `k in d` (dict membership is supported; LIST membership is not — see
# list_membership.zap). Dict *iteration order* is a separate deliberate
# divergence (dict_order.zap); nothing here depends on it. String `in` (substring
# membership) is also exercised.
def mark(bool b) -> int64:
    if b:
        return 1
    return 0


def main():
    auto d = {"a": 1, "b": 2, "c": 3}
    print(len(d))
    print(d["a"])
    print(d["c"])
    print(mark("b" in d))
    print(mark("z" in d))
    auto counts = {10: 100, 20: 200}
    print(counts[20])
    print(mark(10 in counts))
    print(mark(30 in counts))
    print(mark("ell" in "hello"))
    print(mark("zz" in "hello"))
