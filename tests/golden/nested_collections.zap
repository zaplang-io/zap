# Golden fixture: nested generic collections (#625).
# A collection whose element/value is itself a collection (list[list[T]],
# dict[K, list[V]], list[dict[K, V]], dict[K, dict[...]]) must round-trip through
# construction, indexing, iteration, mutation, and read-back. The front-end
# accepted these shapes but codegen collapsed the inner element type to a bare
# pointer, so a read-back dispatched as a string operation and silently produced
# wrong/empty values. Leaf types are scalar.

def make_grid() -> list[list[int64]]:
    return [[1, 2], [3, 4, 5]]

def main():
    # list[list[int64]]: build via a returning fn, index, nested index, len.
    auto rows = make_grid()
    print(len(rows))         # 2
    print(len(rows[0]))      # 2
    print(len(rows[1]))      # 3
    print(rows[0][1])        # 2
    print(rows[1][2])        # 5

    # iterate the outer list, then each inner list; sum all (before mutation).
    auto total = 0
    for r in rows:
        for x in r:
            total = total + x
    print(total)             # 1+2+3+4+5 = 15

    # mutate an inner list in place, then read back.
    rows[0].append(99)
    print(len(rows[0]))      # 3
    print(rows[0][2])        # 99

    # dict[str, list[int64]]: the value is a list.
    auto buckets = {"a": [10, 20], "b": [30]}
    print(buckets["a"][0])   # 10
    print(buckets["a"][1])   # 20
    print(len(buckets["b"])) # 1
    buckets["b"].append(40)
    print(buckets["b"][1])   # 40

    # list[dict[str, int64]]: the element is a dict.
    auto records = [{"x": 1}, {"x": 2, "y": 3}]
    print(records[0]["x"])   # 1
    print(records[1]["y"])   # 3

    # dict[str, dict[str, int64]]: a two-level dict.
    auto grid = {"p": {"q": 7, "r": 8}}
    print(grid["p"]["q"])    # 7
    print(grid["p"]["r"])    # 8

    # three-level nesting: list[list[list[int64]]].
    auto deep = [[[5, 6]], [[7]]]
    print(deep[0][0][1])     # 6
    print(deep[1][0][0])     # 7

    # dict.items() binds the nested value with its real (list) type.
    auto m = {"k": [8, 9]}
    for key, vals in m.items():
        print(len(vals))     # 2
        print(vals[0])       # 8
        print(vals[1])       # 9

    # a comprehension producing a nested list, then read back.
    auto squares = [[i, i * i] for i in [2, 3]]
    print(squares[0][1])     # 4
    print(squares[1][1])     # 9
