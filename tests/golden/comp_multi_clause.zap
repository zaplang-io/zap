# Golden fixture: comprehensions with multiple `for` clauses and interleaved
# `if` filters (#599). Nested `for`s iterate outer-to-inner, left to right, and
# each `if` filters the clauses to its left. Covers a two-`for` list
# comprehension with a trailing filter, a first-clause filter feeding a second
# `for`, a two-`for` dict comprehension, and a two-`for` set comprehension.
# Sums and lengths are order-independent.
def main():
    list[int64] xs = [1, 2, 3]
    list[int64] ys = [1, 2, 3]

    # pairs with x != y: 1*2, 1*3, 2*1, 2*3, 3*1, 3*2 -> sum 22, count 6
    list[int64] prod = [x * y for x in xs for y in ys if x != y]
    auto sp = 0
    for p in prod:
        sp = sp + p
    print(sp)
    print(len(prod))

    # first-clause filter, then a second `for`: x in {1,2}, y in {1,2}
    # -> 1+1, 1+2, 2+1, 2+2 -> sum 12, count 4
    list[int64] sums = [x + y for x in range(3) if x > 0 for y in range(3) if y > 0]
    auto ssum = 0
    for v in sums:
        ssum = ssum + v
    print(ssum)
    print(len(sums))

    # two-`for` dict comprehension: keys 0,1,10,11; values 0,1,1,2 -> sum 4
    dict[int64, int64] grid = {x * 10 + y: x + y for x in range(2) for y in range(2)}
    auto gsum = 0
    for k in grid.keys():
        gsum = gsum + grid[k]
    print(gsum)
    print(len(grid))

    # two-`for` set comprehension: {0,1,2,10,11,12} -> len 6
    set[int64] cells = {x + y for x in [0, 10] for y in [0, 1, 2]}
    print(len(cells))
