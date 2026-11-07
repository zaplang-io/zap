# diff: diverge BUG #776: `a if cond else b` evaluates BOTH branches, not just one
# Intent (CPython parity, epic #374 walled garden): the conditional expression is
# lazy — only the chosen branch runs. `pick(7) if True else pick(99)` should call
# `pick` once (printing "eval" once) and yield 7.
#
# Zap lowers `IfElse` to an eager `Inst::Select` that evaluates BOTH operands
# first (crates/ir/src/lower.rs ~10806), so `pick(99)` also runs — "eval" prints
# twice. Pinned as a documented divergence; when #776 lands the harness flags
# DIVERGENCE-GONE, promote it to `# diff: match` then.
def pick(int64 x) -> int64:
    print("eval")
    return x


def main():
    print(pick(7) if True else pick(99))
