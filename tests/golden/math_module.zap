# Golden fixture: the `math` stdlib module (ticket #98, D-0056). Constants are
# attributes (`math.pi`); functions are libm-backed and operate on float64
# (integer arguments widen). `import math` is accepted but optional. Scalar
# floats print via `%f` (six decimals). Boolean predicates (`isnan`/`isinf`) are
# exercised through `if` rather than printed directly.
import math

def main():
    print(math.sqrt(2.0))  # 1.414214
    print(math.pow(2.0, 10.0))  # 1024.000000
    print(math.floor(3.7))  # 3.000000
    print(math.ceil(3.2))  # 4.000000
    print(math.trunc(-2.9))  # -2.000000
    print(math.hypot(3.0, 4.0))  # 5.000000
    print(math.fabs(-5.5))  # 5.500000
    print(math.fmod(7.0, 3.0))  # 1.000000
    print(math.pi)  # 3.141593
    print(int64(math.log2(8.0)))  # 3
    print(int64(math.round(2.5)))  # 3 (round-half-away-from-zero)
    print(int64(math.sqrt(16)))  # 4 (int argument widens)
    if math.isnan(math.nan) and math.isinf(math.inf):
        print(100)
    else:
        print(0)
