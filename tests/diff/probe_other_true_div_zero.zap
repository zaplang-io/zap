# diff: match
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
