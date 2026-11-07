# diff: match
# The `a if cond else b` conditional expression selects the right branch,
# matching CPython (epic #374, walled garden): it picks `a` when the condition is
# truthy else `b`, nests right-associatively (`x if c1 else y if c2 else z` groups
# as `x if c1 else (y if c2 else z)`), and binds looser than `or`. (Lazy
# evaluation of only the chosen branch is a separate, diverging property — see
# ternary_eval_once.zap and #776.)
def main():
    print(10 if True else 20)
    print(1 if False else 2 if True else 3)
    auto flag = True
    print(0 if flag or False else 5)
    print(99 if 0 else 42)
