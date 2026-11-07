# diff: diverge BUG #741: float(s) on malformed input aborts uncatchably — CONFIRMED by PR #738 CI, core defect B9
# PROBE (epic #374, suspected bug B9): SPEC §9 says `float(s)` "faults" on a
# malformed string, whereas `int(s)` raises a catchable `ValueError` (#456).
# CPython raises a catchable `ValueError` for both, so the handler runs and
# prints `caught`. If Zap's `float(s)` faults uncatchably the process aborts and
# `caught` never prints -> MATCH-FAILED reveals it. If it is catchable this stays
# green and B9 is benign. CPython prints:
#   caught
def main():
    try:
        auto x = float("not-a-number")
        print(x)
    except ValueError as e:
        print("caught")
