# diff: diverge #743: float / 0.0 yields inf instead of ZeroDivisionError — CONFIRMED by PR #738 CI, decision pending
# PROBE (epic #374, §10 "other"): true (float) division by zero. CPython raises
# a catchable `ZeroDivisionError`, so the handler runs and prints `caught`. If
# Zap follows IEEE and yields `inf` instead, the guarded `print` runs and
# `caught` never prints -> MATCH-FAILED reveals an undocumented divergence.
# CPython prints:
#   caught
def main():
    try:
        print(1.0 / 0.0)
    except Exception as e:
        print("caught")
