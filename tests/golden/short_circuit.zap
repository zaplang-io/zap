# Golden fixture: boolean `and` / `or` short-circuit left-to-right, Python-style
# (I#334). The right operand is evaluated only when the left does not already
# decide the result, so the universal guard idiom `bound-check and index` never
# faults, and a side-effecting right operand does not fire when skipped. The
# result is a strict `bool` (D-0022): the surviving operand's truthiness.

# A right operand with an observable effect: it prints a marker when evaluated.
# If short-circuiting works, these markers never appear in the output.
def boom() -> bool:
    print("BOOM")  # must NOT print when short-circuited
    return True

def main():
    list[int64] xs = [1, 2, 3]

    # --- `and` guard: left is False, so `xs[i]` (out of range) is NOT evaluated.
    auto i = 3
    if i < len(xs) and xs[i] == 9:
        print("and-taken")
    else:
        print("and-not-taken")  # expected

    # --- `or` guard: left is True, so `xs[i]` (out of range) is NOT evaluated.
    if i >= len(xs) or xs[i] == 9:
        print("or-taken")  # expected
    else:
        print("or-not-taken")

    # --- `and` short-circuit must skip a side-effecting right operand.
    if False and boom():
        print("unreachable-and")
    else:
        print("and-skipped")  # expected, no BOOM

    # --- `or` short-circuit must skip a side-effecting right operand.
    if True or boom():
        print("or-short")  # expected, no BOOM
    else:
        print("unreachable-or")

    # --- chained `and`: stops at the first falsy operand.
    auto j = 5
    if j > 0 and j < len(xs) and xs[j] == 1:
        print("chain-and-taken")
    else:
        print("chain-and-not-taken")  # expected (j < len(xs) is False)

    # --- chained `or`: stops at the first truthy operand.
    if j < 0 or j >= len(xs) or xs[j] == 1:
        print("chain-or-taken")  # expected (j >= len(xs) is True)
    else:
        print("chain-or-not-taken")

    # --- mixed `a and b or c` parses as `(a and b) or c`.
    # (False and boom()) is False, then `or True` is True; boom() stays skipped.
    if False and boom() or True:
        print("mixed-taken")  # expected, no BOOM
    else:
        print("mixed-not-taken")

    # --- value semantics: `and`/`or` yield a strict bool (D-0022).
    bool a = True and True
    bool b = True and False
    bool c = False or True
    bool d = False or False
    print(str(a))   # True
    print(str(b))   # False
    print(str(c))   # True
    print(str(d))   # False

    print("survived")
