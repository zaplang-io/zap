# Golden (epic #374, B1): str(float) is the shortest round-trip decimal (CPython
# repr), replacing the old 6-significant-digit %g. Covers a repeating fraction,
# the 0.1+0.2 classic, the fixed-vs-exponent threshold (1e15 fixed, 1e16
# exponent; 1e-4 fixed, 1e-5 exponent), signed zero, and the double extremes.
def main():
    print(str(1.0 / 3.0))
    print(str(0.1 + 0.2))
    print(str(1e15))
    print(str(1e16))
    print(str(0.0001))
    print(str(0.00001))
    print(str(-0.0))
    print(str(5e-324))
    print(str(1.7976931348623157e308))
