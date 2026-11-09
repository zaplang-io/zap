# Golden fixture: dict[float64, V] KEYS (#803 Phase 2, D-0138).
#
# `float64` is a first-class dict *key*: a key is canonicalized (`-0.0 -> +0.0`,
# so `0.0` and `-0.0` collide to one slot) and `NaN` is rejected at runtime with a
# catchable ValueError. Covers construct, insert, read back, update-in-place,
# membership (`in`), `del`, `.keys()`/`.values()`, iteration, the `-0.0`/`+0.0`
# collision, and NaN rejection on insert / lookup / `in` / `del` (each caught, and
# the dict left unchanged — no NaN slot leaks). Keys/values format exactly.
import math


def main():
    dict[float64, str] prices = {}
    prices[1.5] = "a"
    prices[2.25] = "b"
    print(len(prices))  # 2
    print(prices[1.5])  # a
    print(prices[2.25])  # b
    prices[1.5] = "z"  # overwrite an existing key's value
    print(prices[1.5])  # z
    print(1.5 in prices)  # True
    print(3.0 in prices)  # False
    del prices[2.25]
    print(len(prices))  # 1
    print(2.25 in prices)  # False

    # `-0.0` and `+0.0` canonicalize to the same key.
    dict[float64, int64] z = {}
    z[0.0] = 7
    print(-0.0 in z)  # True
    z[-0.0] = 9  # overwrites the same slot
    print(z[0.0])  # 9
    print(len(z))  # 1

    # An f64-keyed f64-valued dict + iteration (sums are order-independent).
    dict[float64, float64] m = {1.0: 10.5, 2.0: 20.5}
    float ks = 0.0
    for k in m:
        ks = ks + k
    print(ks)  # 3.0  (1.0 + 2.0)
    float vs = 0.0
    for v in m.values():
        vs = vs + v
    print(vs)  # 31.0  (10.5 + 20.5)

    # NaN is rejected (catchable ValueError) on every key operation, and the dict
    # is left untouched.
    dict[float64, int64] d = {1.0: 1}
    auto raised = 0
    int64 probe = -1
    try:
        probe = d[math.nan]  # lookup
    except ValueError:
        raised = raised + 1
    bool hit = True
    try:
        hit = math.nan in d  # membership
    except ValueError:
        raised = raised + 1
    try:
        d[math.nan] = 5  # insert
    except ValueError:
        raised = raised + 1
    try:
        del d[math.nan]  # delete
    except ValueError:
        raised = raised + 1
    print(raised)  # 4
    print(probe)  # -1    (lookup raised before assigning)
    print(hit)  # True  (membership raised before assigning)
    print(len(d))  # 1     (unchanged — no NaN slot leaked)
