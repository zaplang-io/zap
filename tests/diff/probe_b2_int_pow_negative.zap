# diff: diverge #743: int ** negative yields an integer floor, not a float — CONFIRMED by PR #738 CI, decision pending (B2 / NUM-5)
# PROBE (epic #374, suspected behavior B2, decision input for NUM-5): an
# all-integer `**` with a negative exponent is suspected to yield the integer
# floor of the true power (`2 ** -1` -> 0) rather than CPython's float. This may
# be a deliberate static-typing consequence; the probe records the actual Zap
# result so the maintainer can rule (divergence -> pin as `# diff: diverge`;
# bug -> file + `# diff: diverge BUG #NNN`). CPython prints:
#   0.5
#   0.01
def main():
    print(2 ** (-1))
    print(10 ** (-2))
