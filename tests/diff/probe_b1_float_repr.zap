# diff: diverge BUG #739: float str/print/JSON render via %g (6 sig digits) — CONFIRMED by PR #738 CI, core defect B1
# PROBE (epic #374, suspected bug B1): `str(float)`/bare `print(float)` are
# suspected to render via C `%g` (6 significant digits) instead of CPython's
# shortest round-trip repr. If so, this case FAILS and the MATCH-FAILED report
# shows Zap's actual output (e.g. `0.333333`) next to CPython's. CPython prints:
#   0.3333333333333333
#   0.30000000000000004
#   1000000.0
#   1234567.0
#   123456789.5
# If Zap already matches, the case stays green and becomes a permanent
# float-rendering regression test.
def main():
    print(1 / 3)
    print(0.1 + 0.2)
    print(1e6)
    print(1234567.0)
    print(123456789.5)
