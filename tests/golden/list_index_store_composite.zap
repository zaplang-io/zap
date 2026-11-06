# Golden fixture (companion to list_index_store_int_word): indexed STORES whose
# element kind is NOT in the integer-word family keep their exact-identity check
# — an enum element (`Elem::Enum`, i64 suffix but its own identity), a `str`
# element (`Elem::Ptr`), and a `dict` VALUE (the non-list `check_index_value`
# path). Each stores a value read back from the same collection, the shape that
# regressed in #727, confirming the fix widened only the int-word family and did
# not loosen enum/ptr identity (see the negative coverage in golden.rs comments).
enum Color:
    RED
    GREEN
    BLUE

def main():
    # enum element: read-back type is `Ty::Enum`, so the store keeps `Elem::Enum`
    # identity (storing a different enum or an int here still fails).
    list[Color] cs = []
    cs.append(Color.RED)
    cs.append(Color.GREEN)
    cs.append(Color.BLUE)
    auto d = 0
    cs[d] = cs[2]
    print(cs)  # [Color.BLUE, Color.GREEN, Color.BLUE]

    # str element: `Elem::Ptr` identity, unchanged by the fix.
    list[str] ss = ["x", "y", "z"]
    ss[0] = ss[2]
    print(ss)  # ['z', 'y', 'z']

    # dict VALUE store (the `list = false` arm of check_index_value): a bool read
    # back from a dict[str, bool] value (a bare i64 word) stored into another
    # dict[str, bool] value — the #727 shape on the dict path. Both dicts are
    # single-entry so iteration order is fixed, and the whole dict is printed so
    # the value renders through the container Display protocol (keyed on the
    # destination's `Elem::Bool`) as `True` — a scalar read-back would instead
    # surface the raw i64 word, an unrelated pre-existing rendering gap.
    dict[str, bool] src = {"k": True}
    dict[str, bool] dst = {"k": False}
    dst["k"] = src["k"]
    print(dst)  # {'k': True}
