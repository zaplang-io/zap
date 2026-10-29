# Golden fixture (#385): string truthiness in boolean contexts.
# An empty string is falsy and a non-empty string is truthy (Python / D-0022),
# covering if / else / not / while / and / or / ternary with empty and
# non-empty values.
def main():
    str empty = ""
    str full = "zap"

    # Bare `if` on a non-empty string, and `if not` on an empty one.
    if full:
        print("full-str-truthy")
    if not empty:
        print("empty-str-falsy")

    # `if`/`else`: an empty string takes the else branch.
    if empty:
        print("UNREACHABLE")
    else:
        print("empty-str-else")
    if full:
        print("full-str-if")

    # `while` condition on a string: non-empty enters, empty is skipped.
    while full:
        print("while-str-truthy")
        break
    while empty:
        print("UNREACHABLE")
    print("while-empty-skipped")

    # `and` / `or` operands are strings (Python-style, D-0022).
    if full and full:
        print("and-both-truthy")
    if empty or full:
        print("or-picks-truthy")
    if not (empty and full):
        print("and-short-circuits")

    # Ternary condition on a string.
    str picked = "ternary-full" if full else "ternary-empty"
    print(picked)
    str other = "ternary-full" if empty else "ternary-empty"
    print(other)
