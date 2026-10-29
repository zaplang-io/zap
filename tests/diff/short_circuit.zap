# diff: match
# Boolean and/or short-circuit left-to-right (SPEC §5, D-0022, I#334). The guard
# idiom `i < len(xs) and xs[i]` must not evaluate the guarded operand when the
# guard fails, and a side-effecting right operand must be skipped. CPython is the
# oracle: it short-circuits, so the markers below never print in either language.
def boom() -> bool:
    print("BOOM")
    return True


def main():
    list[int64] xs = [1, 2, 3]
    auto i = 3
    if i < len(xs) and xs[i] == 9:
        print("and-taken")
    else:
        print("and-not-taken")
    if i >= len(xs) or xs[i] == 9:
        print("or-taken")
    else:
        print("or-not-taken")
    if False and boom():
        print("unreachable")
    else:
        print("and-skipped")
    if True or boom():
        print("or-short")
    else:
        print("unreachable")
    if False and boom() or True:
        print("mixed-taken")
    else:
        print("mixed-not-taken")
