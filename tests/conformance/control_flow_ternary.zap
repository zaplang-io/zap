# claim: CTRL-6
# SPEC §5/§6 (#374, #776): the conditional expression `a if cond else b`
# evaluates only the chosen branch — the non-taken branch's side effects (and
# faults) never happen, so a guard like `xs[0] if len(xs) > 0 else -1` returns
# the default on an empty list instead of raising.
def mark(int64 x) -> int64:
    print("eval", x)
    return x


def main():
    print(mark(1) if True else mark(2))
    list[int64] xs = []
    print(xs[0] if len(xs) > 0 else -1)
