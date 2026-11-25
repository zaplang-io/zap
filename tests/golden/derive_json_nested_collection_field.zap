# @derive(Json) with NESTED-collection fields (#1533, epic #1529): a collection
# whose element/value is itself a collection — list[list[T]], dict[str, list[T]],
# list[dict[str, T]], and a struct leaf (list[list[Nested]]). The encode/decode
# recursion recovers the nested element's full type from the Elem::Nested interner
# (ctx.nested) and recurses through the same per-element codec, so arbitrary JSON
# array/object nesting round-trips.
#
# A nested-collection-field struct cannot @derive(Eq) (D-0049) and dict order is
# unspecified (#1066), so the round-trip is proven by nested index / key
# inspection. `list[Optional[...]]` stays out of scope (a value-position Optional
# is a separate follow-up).

@derive(Json)
struct Item:
    int64 id

@derive(Json)
struct N:
    list[list[int64]] grid
    dict[str, list[int64]] groups
    list[dict[str, int64]] rows
    list[list[Item]] items2d

def main():
    auto n = N([[1, 2], [3]], {"a": [4, 5]}, [{"x": 7}], [[Item(9)]])
    auto d = N.from_json(n.to_json())
    print(len(d.grid))  # 2
    print(d.grid[0][1])  # 2
    print(d.grid[1][0])  # 3
    print(d.groups["a"][1])  # 5
    print(len(d.groups["a"]))  # 2
    print(d.rows[0]["x"])  # 7
    print(len(d.rows))  # 1
    print(d.items2d[0][0].id)  # 9

    auto e = N([], {}, [], [])
    auto d2 = N.from_json(e.to_json())
    print(len(d2.grid))  # 0
    print(len(d2.groups))  # 0
    print(len(d2.rows))  # 0
    print(len(d2.items2d))  # 0
