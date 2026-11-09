# Golden fixture: dict[K, float64] VALUES (#800 Phase 1).
#
# `float64` is a first-class dict *value* everywhere `int64` already is: only the
# key is hashed, so a float value just shares the uniform 8-byte dict word (a bit
# reinterpret, like list[float] in I#569). Covers construct, insert, read back,
# update-in-place, membership (`in` is over keys), len, `del`, `.values()`, and an
# int64-keyed float dict. Values are chosen to format exactly (halves/quarters).
def main():
    dict[str, float64] scores = {}
    scores["alice"] = 90.5
    scores["bob"] = 77.0
    print(len(scores))  # 2
    print(scores["alice"])  # 90.5
    print(scores["bob"])  # 77.0
    scores["alice"] = 95.25  # overwrite an existing key's value
    print(scores["alice"])  # 95.25
    print("alice" in scores)  # True
    print("carol" in scores)  # False
    # `.values()` yields a list[float64]; the sum is order-independent.
    float total = 0.0
    for v in scores.values():
        total = total + v
    print(total)  # 95.25 + 77.0 = 172.25
    del scores["bob"]
    print(len(scores))  # 1
    print("bob" in scores)  # False

    # An int64-keyed float dict (literal with float values).
    dict[int64, float64] t = {1: 1.5, 2: 2.5, 3: 3.0}
    print(t[2])  # 2.5
    print(len(t))  # 3
    float ts = 0.0
    for w in t.values():
        ts = ts + w
    print(ts)  # 1.5 + 2.5 + 3.0 = 7.0
