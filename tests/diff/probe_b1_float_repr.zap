# diff: match
# Regression (epic #374, FIXED in #739/B1): str / print / f-string / JSON now
# render a float as its shortest round-trip decimal (CPython repr), not C %g's
# 6 significant digits. Matches CPython exactly.
def main():
    print(1 / 3)
    print(0.1 + 0.2)
    print(1e6)
    print(1234567.0)
    print(123456789.5)
