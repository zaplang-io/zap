# Golden fixture (#382): container truthiness in boolean contexts.
# A list or dict is falsy when empty and truthy otherwise (Python / D-0022),
# covering if / else / not / while / and / or with empty and non-empty values.
def main():
    list[int64] empty = []
    list[int64] full = [1, 2, 3]
    dict[str, int64] ed = {}
    dict[str, int64] fd = {"a": 1}

    # Bare `if` on a non-empty list, and `if not` on an empty one.
    if full:
        print("full-list-truthy")
    if not empty:
        print("empty-list-falsy")

    # `if`/`else` on dicts.
    if ed:
        print("UNREACHABLE")
    else:
        print("empty-dict-falsy")
    if fd:
        print("full-dict-truthy")

    # `while` condition on a container: non-empty enters, empty is skipped.
    while full:
        print("while-list-truthy")
        break
    while empty:
        print("UNREACHABLE")
    print("while-empty-skipped")

    # `and` / `or` operands are containers (Python-style, D-0022).
    if full and fd:
        print("and-both-truthy")
    if empty or full:
        print("or-picks-truthy")
    if not (empty and full):
        print("and-short-circuits")
