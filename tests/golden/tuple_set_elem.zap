# Golden fixture: tuples as set elements (#680, D-0049). Set elements are hashed
# and compared exactly like dict keys, so a tuple element uses the same
# synthesized structural `__hash__`/`__eq__`. Exercises literal construction
# with duplicates (dedup by structural equality), `len`, membership (`in`,
# present and absent), `.add` (new and already-present), and nested tuples.
def main():
    # Literal with a structural duplicate -> deduplicated.
    auto s = {(1, 2), (3, 4), (1, 2)}
    print(len(s))            # 2
    if (1, 2) in s:
        print(1)             # 1
    else:
        print(0)
    if (5, 6) in s:
        print(1)
    else:
        print(0)             # 0
    s.add((5, 6))            # new element
    s.add((1, 2))            # already present -> no-op
    print(len(s))            # 3
    if (5, 6) in s:
        print(1)             # 1
    else:
        print(0)

    # (int, str) tuple elements: str compared by value.
    auto ss = {(1, "x"), (2, "y"), (1, "x")}
    print(len(ss))           # 2
    if (2, "y") in ss:
        print(1)             # 1
    else:
        print(0)
    if (2, "z") in ss:
        print(1)
    else:
        print(0)             # 0

    # Nested tuple elements.
    auto sn = {((1, 2), 3), ((1, 2), 3), ((1, 2), 4)}
    print(len(sn))           # 2
    if ((1, 2), 4) in sn:
        print(1)             # 1
    else:
        print(0)
    if ((1, 2), 5) in sn:
        print(1)
    else:
        print(0)             # 0
