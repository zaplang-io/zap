# Golden fixture: dict enumeration, membership, and shallow copy (ticket #299)
# over an int-keyed dict. Iteration order is unspecified (hash order), so every
# check is written order-independently: counts, value sums via lookup, and
# membership. Covers `for k in d`, keys()/values(), `for k, v in d.items()`,
# `k in d` (true/false), and copy() independence.
def main():
    auto sq = {1: 1, 2: 4, 3: 9, 4: 16}

    # `for k in d` iterates keys; sum the values by lookup (order-independent).
    auto n = 0
    auto tot = 0
    for k in sq:
        n = n + 1
        tot = tot + sq[k]
    print(n)  # 4
    print(tot)  # 30

    # keys()/values() as lists.
    print(len(sq.keys()))  # 4
    auto vsum = 0
    for v in sq.values():
        vsum = vsum + v
    print(vsum)  # 30

    # items() + tuple-destructuring: sum keys and values independently.
    auto ksum = 0
    auto vsum2 = 0
    for k, v in sq.items():
        ksum = ksum + k
        vsum2 = vsum2 + v
    print(ksum)  # 10
    print(vsum2)  # 30

    # `k in d` key membership.
    if 2 in sq:
        print(1)  # 1
    if 7 in sq:
        print(1)
    else:
        print(0)  # 0

    # Shallow copy: extending the copy leaves the original untouched.
    auto c = sq.copy()
    c[5] = 25
    print(len(sq))  # 4
    print(len(c))  # 5
    if 5 in sq:
        print(1)
    else:
        print(0)  # 0
    if 5 in c:
        print(1)  # 1
    else:
        print(0)
