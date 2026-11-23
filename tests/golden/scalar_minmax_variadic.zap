# Golden fixture: variadic min/max and exact sum (#1284).
#
# `min`/`max` take two or more scalar arguments and fold a compare + select,
# keeping integers exact. `sum(list)` reduces at the element type, so summing
# int64 values stays exact (never routed through float).

def main():
    # Variadic integer min/max (folded left to right).
    print(min(5, 2, 8, 1))  # 1
    print(max(5, 2, 8, 1))  # 8
    print(min(3, 3, 3))  # 3
    print(max(-5, -2, -9))  # -2
    print(min(-5, -2, -9))  # -9

    # Variadic float min/max.
    print(min(1.5, 2.5, 0.5))  # 0.5
    print(max(1.5, 2.5, 0.5))  # 2.5

    # Exact sum over an int64 list (binds into an int slot).
    int64 s = sum([10, 20, 30, 40])
    print(s)  # 100

    # Sum over a float list.
    float64 fs = sum([1.5, 2.5, 1.0])
    print(fs)  # 5.0

    # Exact: summing values above 2^53 stays exact — a float accumulator would
    # round 9007199254740992 + 1 back to 9007199254740992.
    int64 big = sum([9007199254740992, 1, 1])
    print(big)  # 9007199254740994
