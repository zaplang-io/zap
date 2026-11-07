# diff: match
# Float literal forms evaluate to the same value as CPython (epic #374, walled
# garden): leading-dot `.5`, trailing-dot `1.`, exponent `1e-3` / `2.5e3`, and
# `_` digit grouping in both the integer and fractional parts. Rendered via str()
# so the shortest round-trip value (B1) is visible.
def main():
    print(str(.5))
    print(str(1.))
    print(str(1e-3))
    print(str(1_000.0))
    print(str(2.5e3))
    print(str(1_0.0_1))
