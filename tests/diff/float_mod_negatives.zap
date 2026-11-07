# diff: diverge BUG #751: float `%` uses truncated frem, not floored modulo
# Intent (CPython parity, epic #374 walled garden): float `%` should take the
# sign of the divisor (floored modulo), consistent with floored float `//`, so
# `a == (a // b) * b + (a % b)` holds. CPython prints -0.5, 0.5, 2.0 below.
#
# Zap currently lowers float `%` to raw LLVM `frem` (C fmod — truncated toward
# zero, sign of the dividend), so it prints 1.5, -1.5, -1.0 instead. The integer
# `%` path was already corrected (zap_imod, D-0024) but the float path was not.
# Pinned as a documented divergence; when #751 lands, the harness flags this as
# DIVERGENCE-GONE — promote it back to `# diff: match` then. (Same-sign operands
# already agree; see the matching cases in float_floor_div.zap.)
def main():
    print(str(5.5 % -2))
    print(str(-5.5 % 2))
    print(str(-7.0 % 3.0))
