# claim: COMP-4
# SPEC §8.1 (#596): a generator expression `(x for x in xs)` is a LAZY iterator,
# not a materialized collection. It yields each element on demand and is consumed
# once; `sum(...)` drains it with no intermediate list, and a genexpr bound to a
# name is a standalone iterator. Output is reduced to scalars.
def main():
    # sum over a genexpr as the sole call argument: 0+1+4+9+16 = 30
    print(sum((x * x for x in range(5))))

    # a genexpr bound to a name, consumed once by a for-loop: 3 + 4 = 7
    list[int64] xs = [1, 2, 3, 4]
    auto g = (y for y in xs if y > 2)
    auto s = 0
    for z in g:
        s = s + z
    print(s)
