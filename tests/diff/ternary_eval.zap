# diff: match
# The `a if cond else b` conditional expression matches CPython (epic #374,
# walled garden): it selects one branch, nests right-associatively
# (`x if c1 else y if c2 else z` groups as `x if c1 else (y if c2 else z)`),
# binds looser than `or`, and evaluates only the chosen branch exactly once —
# pinned with a side-effecting function whose print shows which branch ran.
def pick(int64 x) -> int64:
    print("eval")
    return x


def main():
    print(10 if True else 20)
    print(1 if False else 2 if True else 3)
    print(pick(7) if True else pick(99))
    auto flag = True
    print(0 if flag or False else 5)
