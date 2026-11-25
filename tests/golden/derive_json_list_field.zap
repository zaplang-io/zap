# @derive(Json) with `list[T]` fields (#1530, epic #1529). The built-in codec
# now encodes a `list` field as a JSON array and decodes a JSON array back into a
# list, for element types int / float / str / bytes / nested @derive(Json) struct.
# Encode string-builds `[e0,e1,…]` recursing into the per-element encoder; decode
# reads the DOM array (`__json_as_array`) and decodes each element.
#
# A struct with a `list` field cannot `@derive(Eq)` (a list is not hashable,
# D-0049), so — like `json_derive_optional.zap` — this proves the round-trip by
# field inspection (lengths + indexed elements) rather than `==`. Covers a scalar
# list, a str list, a nested-struct list, and the empty-list case.

@derive(Json)
struct Item:
    int64 id
    str tag

@derive(Json)
struct Bag:
    list[int64] nums
    list[str] names
    list[Item] items

def main():
    auto b = Bag([1, 2, 3], ["a", "b"], [Item(7, "x"), Item(8, "y")])
    # Encode is a compact JSON array, fields in declaration order.
    print(b.to_json())  # {"nums":[1,2,3],"names":["a","b"],"items":[{"id":7,"tag":"x"},{"id":8,"tag":"y"}]}

    # Round-trip through the built-in codec, inspected field by field.
    auto d = Bag.from_json(b.to_json())
    print(len(d.nums))  # 3
    print(d.nums[0])  # 1
    print(d.nums[2])  # 3
    print(len(d.names))  # 2
    print(d.names[0])  # a
    print(d.names[1])  # b
    print(len(d.items))  # 2
    print(d.items[0].id)  # 7
    print(d.items[0].tag)  # x
    print(d.items[1].id)  # 8
    print(d.items[1].tag)  # y

    # Empty lists round-trip to empty lists.
    auto e = Bag([], [], [])
    print(e.to_json())  # {"nums":[],"names":[],"items":[]}
    auto d2 = Bag.from_json(e.to_json())
    print(len(d2.nums))  # 0
    print(len(d2.names))  # 0
    print(len(d2.items))  # 0
