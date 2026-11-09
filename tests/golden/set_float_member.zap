# Golden fixture: set[float64] MEMBERS (#803 Phase 2, D-0138).
#
# `float64` is a first-class set *member*: a member is canonicalized (`-0.0 ->
# +0.0`, so `0.0` and `-0.0` are one member) and `NaN` is rejected at runtime with
# a catchable ValueError. Covers construct, add, duplicate (no effect), membership
# (`in`), len, iteration, a set literal, the `-0.0`/`+0.0` collision, and NaN
# rejection on add / `in` (each caught, and the set left unchanged). `set` has no
# element-removal entry point yet, so the NaN-on-delete rule rides the dict `del`
# path (see dict_float_key.zap and D-0138).
import math


def main():
    set[float64] seen = set[float64]()
    seen.add(1.5)
    seen.add(2.25)
    seen.add(1.5)  # duplicate — no effect
    print(len(seen))  # 2
    print(1.5 in seen)  # True
    print(3.0 in seen)  # False

    # `-0.0` and `+0.0` are the same member.
    seen.add(0.0)
    print(-0.0 in seen)  # True
    seen.add(-0.0)  # already present (canonicalizes to +0.0)
    print(len(seen))  # 3

    # Iteration (order unspecified) — the sum is order-independent.
    float total = 0.0
    for x in seen:
        total = total + x
    print(total)  # 3.75  (1.5 + 2.25 + 0.0)

    # A set literal of floats, with a duplicate.
    set[float64] lit = {1.0, 2.0, 2.0, 3.0}
    print(len(lit))  # 3

    # NaN is rejected (catchable ValueError) on add and membership; the set is
    # left untouched.
    auto raised = 0
    try:
        seen.add(math.nan)  # add
    except ValueError:
        raised = raised + 1
    bool hit = True
    try:
        hit = math.nan in seen  # membership
    except ValueError:
        raised = raised + 1
    print(raised)  # 2
    print(hit)  # True  (membership raised before assigning)
    print(len(seen))  # 3     (unchanged — no NaN member leaked)
