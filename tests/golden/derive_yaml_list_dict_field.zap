# @derive(Yaml) with `list[T]` and `dict[str, V]` fields (#1532, epic #1529).
# YAML 1.2 is a JSON superset, so the built-in codec emits a collection field in
# compact JSON flow (`nums: [1,2,3]`, `scores: {"x":9}`) — valid YAML that
# round-trips through `from_yaml` (which shares the `from_json_value` decode core
# with `@derive(Json)`). Covers int / str element and value types, both lists and
# str-keyed dicts, and the empty case.
#
# Scope: a nested-struct element/value in a yaml collection is a follow-up (its
# flow encode would route through `to_json`, which `@derive(Yaml)` does not emit).
# A struct with a collection field cannot `@derive(Eq)` (D-0049) and dict order is
# unspecified (#1066), so the round-trip is proven by keyed/indexed inspection.

@derive(Yaml)
struct W:
    list[int64] nums
    list[str] names
    dict[str, int64] scores

def main():
    auto w = W([1, 2, 3], ["a", "b"], {"x": 9})
    auto d = W.from_yaml(w.to_yaml())
    print(len(d.nums))  # 3
    print(d.nums[0])  # 1
    print(d.nums[2])  # 3
    print(len(d.names))  # 2
    print(d.names[1])  # b
    print(d.scores["x"])  # 9
    print(len(d.scores))  # 1

    auto e = W([], [], {})
    auto d2 = W.from_yaml(e.to_yaml())
    print(len(d2.nums))  # 0
    print(len(d2.names))  # 0
    print(len(d2.scores))  # 0
