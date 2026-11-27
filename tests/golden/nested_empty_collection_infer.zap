# Golden fixture: a collection display whose element type cannot be read off its
# FIRST element — because that element is an empty/ambiguous collection literal —
# is inferred from its non-empty siblings, exactly as sema already types it
# (#1708).
#
# `[[], ["group", "switch"]]` has no type annotation, so lowering infers the
# element kind itself. It used to read the kind off the first element: an empty
# `[]` defaults to `list[int64]`, so the outer list became `list[list[int64]]`,
# and the `list[str]` sibling was then rejected at lowering with
# `error[ZAP6001]: list element expects i64, found ptr` — even though sema had
# already accepted the program and joined the siblings to `list[list[str]]`.
# Lowering now agrees with sema's join: an empty leading element contributes no
# element-type constraint, and a non-empty sibling supplies it. The empty member
# is covered both BEFORE and AFTER a non-empty sibling, and between two of them.
#
# The dict VALUE position has the identical gap (`{1: [], 2: ["x", "y"]}`): the
# empty `[]` value no longer locks the value kind to `list[int64]`.
#
# Scalar-element displays (`[1, 2, 3]`) are unaffected — their first element is
# already a usable anchor, so their lowering is byte-for-byte unchanged.

def main():
    # 1. The exact issue repro: a leading empty element, then a `list[str]`.
    #    Inferred as `list[list[str]]`; both lengths print.
    for entry in [[], ["group", "switch"]]:
        print(len(entry))  # 0 then 2

    # 2. The empty member AFTER a non-empty sibling (the other ordering).
    for entry in [["group", "switch"], []]:
        print(len(entry))  # 2 then 0

    # 3. An empty member BETWEEN two non-empty siblings.
    for entry in [["a"], [], ["b", "c", "d"]]:
        print(len(entry))  # 1, 0, 3

    # 4. The inferred inner element really is `str`: iterating the non-empty
    #    sibling yields `str` loop values (printed bare, no quotes). The empty
    #    sibling contributes no iterations.
    for entry in [[], ["group", "switch"]]:
        for s in entry:
            print(s)  # group then switch

    # 5. The literal's inferred shape, rendered: an empty inner list prints `[]`
    #    and the `str` elements print quoted inside the list repr.
    print([[], ["group", "switch"]])  # [[], ['group', 'switch']]

    # 6. Deep nesting — the leading empty is two levels down. Inferred as
    #    `list[list[list[str]]]`; the empty `[[]]` row and the `[["a", "b"]]` row
    #    both have length 1.
    for row in [[[]], [["a", "b"]]]:
        print(len(row))  # 1 then 1
    print([[[]], [["a", "b"]]])  # [[[]], [['a', 'b']]]

    # 7. The dict counterpart: a leading empty list VALUE, keyed access (dict
    #    iteration order is unspecified, so index by key, never iterate).
    print(len({1: [], 2: ["x", "y"]}[1]))  # 0
    print(len({1: [], 2: ["x", "y"]}[2]))  # 2
    print({1: [], 2: ["x", "y"]}[2][0])  # x

    # 8. A nested-dict VALUE with a leading empty dict.
    print(len({"a": {}, "b": {"k": "v"}}["a"]))  # 0
    print(len({"a": {}, "b": {"k": "v"}}["b"]))  # 1
    print({"a": {}, "b": {"k": "v"}}["b"]["k"])  # v
