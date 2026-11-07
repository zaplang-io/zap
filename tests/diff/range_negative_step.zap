# diff: diverge BUG #759: for-range ignores step sign (negative step never iterates)
# Intent (CPython parity, epic #374 walled garden): `range(5, 0, -1)` counts down
# 5,4,3,2,1 (sum 15) and `range(10, 0, -2)` yields 10,8,6,4,2 (sum 30).
#
# Zap's for-range lowering hard-codes the loop condition to `cur < stop`
# regardless of step sign (crates/ir/src/lower.rs ~9248), so a negative step
# starts already-false and the body never runs — both sums are 0 here. Pinned as
# a documented divergence; when #759 lands the harness flags this DIVERGENCE-GONE,
# promote it to `# diff: match` then. (The step==0 infinite-loop case from #759 is
# deliberately omitted — it would hang.)
def main():
    auto a = 0
    for i in range(5, 0, -1):
        a = a + i
    print(a)
    auto b = 0
    for i in range(10, 0, -2):
        b = b + i
    print(b)
