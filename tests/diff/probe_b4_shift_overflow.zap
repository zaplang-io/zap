# diff: diverge BUG #740: unmasked shift, count >= 64 is poison/garbage — CONFIRMED by PR #738 CI, core defect B4
# PROBE (epic #374, suspected bug B4): `<<`/`>>` are suspected to lower to raw
# LLVM shifts with the count unmasked, so a count >= 64 is poison (undefined).
# Zap's default int is a wrapping int64 (D-0019), so `1 << 64` cannot equal
# CPython's big result regardless; the point of the probe is to observe whether
# Zap produces a *defined* value or garbage/poison. The MATCH-FAILED report
# shows Zap's actual output. CPython prints:
#   18446744073709551616
#   -1
def main():
    print(1 << 64)
    print(-1 >> 100)
