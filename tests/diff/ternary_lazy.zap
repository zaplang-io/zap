# diff: match
# `a if cond else b` evaluates only the chosen branch, in either direction, and
# — crucially — never evaluates the non-taken branch, so a guard idiom like
# `xs[0] if len(xs) > 0 else -1` returns the default on an empty list instead of
# faulting on `xs[0]` (#374, fixed by #776). CPython is the oracle.
def pick(int64 x) -> int64:
    print("p")
    return x


def main():
    print(pick(1) if True else pick(2))
    print(pick(3) if False else pick(4))
    list[int64] xs = []
    print(xs[0] if len(xs) > 0 else -1)
    list[int64] ys = [42]
    print(ys[0] if len(ys) > 0 else -1)
