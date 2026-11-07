# diff: match
# The conditional expression `a if cond else b` evaluates ONLY the chosen branch
# (#374, fixed by #776), matching CPython: `pick(7) if True else pick(99)` calls
# `pick` a single time ("eval" prints once) and yields 7.
def pick(int64 x) -> int64:
    print("eval")
    return x


def main():
    print(pick(7) if True else pick(99))
