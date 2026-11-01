# Golden fixture: the `math` stdlib module (ticket #98, D-0056). Constants are
# attributes (`math.pi`); functions are libm-backed and operate on float64
# (integer arguments widen). `import math` is accepted but optional. Scalar
# floats print via `str()`'s `%g`-style formatter (I#493): six significant
# figures, integral values keeping a trailing `.0`. Boolean predicates
# (`isnan`/`isinf`) are exercised through `if` rather than printed directly.
import math

def main():
    print(math.sqrt(2.0))  # 1.41421
    print(math.pow(2.0, 10.0))  # 1024.0
    print(math.floor(3.7))  # 3.0
    print(math.ceil(3.2))  # 4.0
    print(math.trunc(-2.9))  # -2.0
    print(math.hypot(3.0, 4.0))  # 5.0
    print(math.fabs(-5.5))  # 5.5
    print(math.fmod(7.0, 3.0))  # 1.0
    print(math.pi)  # 3.14159
    print(int64(math.log2(8.0)))  # 3
    print(int64(math.round(2.5)))  # 3 (round-half-away-from-zero)
    print(int64(math.sqrt(16)))  # 4 (int argument widens)
    if math.isnan(math.nan) and math.isinf(math.inf):
        print(100)
    else:
        print(0)
