# claim: BOOL-1
# SPEC §5 (#334, D-0022): `and`/`or` short-circuit left-to-right — the right
# operand is evaluated only when the left does not already decide the result.
# A side-effecting right operand (`boom()`) must therefore never run on the
# short-circuited path; `boom` printing would be observable, and never does.
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
