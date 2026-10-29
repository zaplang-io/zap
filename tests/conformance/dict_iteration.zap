# claim: DICT-2
# SPEC §18 (#299, D-0056): a dict iterates its keys (`for k in d`), exposes
# `values()`, and supports `k in d` membership. Built-in dict iteration order is
# UNSPECIFIED (hash-ordered, D-0056), so every observation here is written
# order-independently — counts, a value sum, and membership — never a sequence.
def main():
    auto sq = {1: 1, 2: 4, 3: 9, 4: 16}
    auto n = 0
    auto tot = 0
    for k in sq:
        n = n + 1
        tot = tot + sq[k]
    print(n)
    print(tot)
    auto vsum = 0
    for v in sq.values():
        vsum = vsum + v
    print(vsum)
    if 2 in sq:
        print("has-2")
    if 7 in sq:
        print("has-7")
    else:
        print("no-7")
