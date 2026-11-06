# Golden fixture: `char` collection elements, keys, and values render
# Python-style `'c'` (#709). Before `char` became a distinct IR type it shared
# `Ty::I32` with `int32`, so a char in a collection rendered as its decimal
# codepoint (`[97, 98]`); now the element kind is `Elem::Char` (a char-tagged
# i64 word — byte-identical storage/hashing to an integer element, distinguished
# only for rendering), so every container position renders the quoted char.
# Dicts and sets iterate in HASH order, so each dict/set here is single-entry to
# keep the golden order-independent; the ordered list covers multiple elements.
def main():
    auto s = "ab"
    # List element kind is inferred from the element VALUES (`s[i]` is a char),
    # so a plain list literal of chars is a `list[char]`.
    list[char] cs = [s[0], s[1]]
    print(cs)  # ['a', 'b']
    # A comprehension over a string yields chars, too.
    list[char] comp = [c for c in s]
    print(comp)  # ['a', 'b']
    set[char] one = {chr(97)}
    print(one)  # {'a'}
    # char as a dict KEY renders quoted; the int64 value renders decimal.
    dict[char, int64] dk = {char(120): 1}
    print(dk)  # {'x': 1}
    # char as a dict VALUE renders quoted; the str key renders quoted.
    dict[str, char] dv = {"k": char(121)}
    print(dv)  # {'k': 'y'}
    # str() matches print for a container.
    print(str(cs))  # ['a', 'b']
    # An int32 list is unaffected: it renders decimals (the conflation is gone).
    list[int32] ns = [97, 98]
    print(ns)  # [97, 98]
