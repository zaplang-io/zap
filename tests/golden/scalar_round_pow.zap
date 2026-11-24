# Golden fixture: Python built-in round() / pow() (#1381).
#
# round() uses banker's rounding (ties-to-even), distinct from math.round
# (half-away-from-zero): round(float) -> int64, round(float, n) -> float64,
# round(int[, n]) -> int (power-of-ten rounding for n<0). An out-of-range
# round raises a catchable ArithmeticError.
#
# pow(b, e) is exactly `b ** e` over the numeric tower; pow(b, e, mod) is
# integer modular exponentiation whose result takes the sign of the modulus.
# A zero modulus or a negative exponent in the 3-arg form raises ValueError;
# a negative integer exponent in the 2-arg form raises ValueError (int ** int
# stays int).

def main():
    # round(float) -> int64, ties-to-even.
    print(round(2.5))  # 2
    print(round(3.5))  # 4
    print(round(0.5))  # 0
    print(round(1.5))  # 2
    print(round(2.4))  # 2
    print(round(2.6))  # 3
    print(round(-2.5))  # -2

    # round(float, ndigits) -> float64.
    print(round(3.14159, 2))  # 3.14
    print(round(3.14159, 4))  # 3.1416
    print(round(2.675, 2))  # 2.67
    print(round(1.005, 2))  # 1.0  (binary-float caveat; matches CPython)
    print(round(2.5, 0))  # 2.0
    print(round(12345.0, -2))  # 12300.0

    # round(int) is identity; round(int, n<0) rounds to a power of ten (even).
    print(round(7))  # 7
    print(round(-7))  # -7
    print(round(12345, -2))  # 12300
    print(round(12350, -2))  # 12400   (quotient 123 odd -> up to even)
    print(round(12450, -2))  # 12400   (quotient 124 even -> stays)
    print(round(-12350, -2))  # -12400
    print(round(50, -2))  # 0   (tie -> even 0)
    print(round(150, -2))  # 200   (tie, quotient 1 odd -> 2)

    # round overflow -> catchable ArithmeticError.
    try:
        print(round(1.0e19))
    except ArithmeticError:
        print("round-overflow")  # round-overflow

    # pow(b, e) == b ** e: int-exact and float via the numeric tower.
    print(pow(2, 10))  # 1024
    print(pow(5, 3))  # 125
    print(pow(2, 0))  # 1
    print(pow(2, 62))  # 4611686018427387904
    print(pow(2.0, 10.0))  # 1024.0

    # pow(b, e, mod): integer modular exponentiation (sign of the modulus).
    print(pow(2, 10, 1000))  # 24
    print(pow(3, 4, 5))  # 1
    print(pow(10, 9, 7))  # 6
    print(pow(2, 0, 1))  # 0
    print(pow(-2, 3, 7))  # 6
    print(pow(2, 3, -7))  # -6

    # 3-arg pow faults -> ValueError.
    try:
        print(pow(5, 3, 0))
    except ValueError:
        print("pow-mod-zero")  # pow-mod-zero
    try:
        print(pow(2, -1, 5))
    except ValueError:
        print("pow-neg-exp")  # pow-neg-exp

    # 2-arg pow with a negative integer exponent raises (int ** stays int).
    try:
        print(pow(2, -1))
    except ValueError:
        print("pow-neg")  # pow-neg
