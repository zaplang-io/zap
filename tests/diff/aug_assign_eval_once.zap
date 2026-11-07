# diff: match
# An augmented-assignment target with a side-effecting subscript is evaluated
# exactly once (epic #374, walled garden), like CPython: `xs[idx()] += 5` calls
# `idx()` a single time, so `idx` prints once and `xs[0]` becomes 15.
def idx() -> int64:
    print("idx")
    return 0


def main():
    list[int64] xs = [10, 20]
    xs[idx()] += 5
    print(xs[0])
