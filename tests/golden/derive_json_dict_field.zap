# @derive(Json) with `dict[str, V]` fields (#1531, epic #1529). The built-in
# codec now encodes a str-keyed dict as a JSON object and decodes a JSON object
# back into a dict, for value types int / float / str / bytes / nested
# @derive(Json) struct. A JSON object has string keys, so only `dict[str, V]` is
# supported; a non-str key is a clean compile error.
#
# A struct with a `dict` field cannot `@derive(Eq)` (a dict is not hashable,
# D-0049), and dict iteration order is unspecified (#1066), so this golden proves
# the round-trip by **keyed inspection** of the decoded value (indexing by key +
# lengths) rather than by `==` or by printing the encoded object. Covers a scalar
# value, a str value, a nested-struct value, and the empty-dict case.

@derive(Json)
struct Item:
    int64 id

@derive(Json)
struct M:
    dict[str, int64] scores
    dict[str, str] tags
    dict[str, Item] items

def main():
    auto m = M({"a": 1, "b": 2}, {"k": "v"}, {"i": Item(7)})
    # Round-trip through the built-in codec, inspected by key (order-independent).
    auto d = M.from_json(m.to_json())
    print(d.scores["a"])  # 1
    print(d.scores["b"])  # 2
    print(len(d.scores))  # 2
    print(d.tags["k"])  # v
    print(len(d.tags))  # 1
    print(d.items["i"].id)  # 7
    print(len(d.items))  # 1

    # Empty dicts round-trip to empty dicts.
    auto e = M({}, {}, {})
    auto d2 = M.from_json(e.to_json())
    print(len(d2.scores))  # 0
    print(len(d2.tags))  # 0
    print(len(d2.items))  # 0
