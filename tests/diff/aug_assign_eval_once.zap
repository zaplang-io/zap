# diff: diverge BUG #750: aug-assign to a subscript target is unsupported by codegen
# Intent (CPython parity, epic #374 walled garden): an augmented-assignment target
# with a side-effecting subscript is evaluated EXACTLY ONCE, so `xs[idx()] += 5`
# calls `idx()` a single time — `idx` prints once and `xs[0]` becomes 15.
#
# Zap currently cannot compile this: `Stmt::AugAssign` lowering only handles a bare
# `Name` target, so `xs[idx()] += 5` fails with "only simple variable aug-assignment
# is supported by codegen yet" (exit 1, empty stdout) while CPython prints "idx\n15".
# Pinned as a documented divergence; when #750 lands, the harness flags this as
# DIVERGENCE-GONE — promote it back to `# diff: match` then.
def idx() -> int64:
    print("idx")
    return 0


def main():
    list[int64] xs = [10, 20]
    xs[idx()] += 5
    print(xs[0])
