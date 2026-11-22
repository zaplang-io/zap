# Golden fixture: scalar abs / divmod (#1283).
#
# `abs` keeps integers exact (never via float): a signed int stays int64, an
# unsigned value is its own magnitude, and abs(INT64_MIN) — whose magnitude 2^63
# has no int64 result — raises a catchable ArithmeticError rather than wrapping.
# `divmod(a, b)` returns `(a // b, a % b)` using the same floored division and
# modulo as the operators, for every sign; a zero divisor raises
# ZeroDivisionError.

def main():
    # abs of signed ints, exact.
    int64 a = -5
    print(abs(a))  # 5
    print(abs(3))  # 3
    int32 n = -1
    print(abs(n))  # 1
    int64 lo = -9223372036854775807
    print(abs(lo))  # 9223372036854775807

    # abs of floats (f64 magnitude).
    float64 f = -2.5
    print(abs(f))  # 2.5
    print(abs(-0.5))  # 0.5

    # abs of an unsigned value is identity.
    uint64 u = 7
    print(abs(u))  # 7

    # abs(INT64_MIN) overflows int64 -> catchable ArithmeticError.
    int64 mn = -9223372036854775807 - 1
    try:
        print(abs(mn))
    except ArithmeticError:
        print("abs-overflow")  # abs-overflow

    # divmod agrees with floored // and % for every sign combination.
    auto q, r = divmod(17, 5)
    print(q)  # 3
    print(r)  # 2
    auto q2, r2 = divmod(-17, 5)
    print(q2)  # -4
    print(r2)  # 3
    auto q3, r3 = divmod(17, -5)
    print(q3)  # -4
    print(r3)  # -3

    # divmod of floats.
    auto fq, fr = divmod(7.5, 2.0)
    print(fq)  # 3.0
    print(fr)  # 1.5

    # divmod of unsigned values.
    uint64 ua = 17
    uint64 ub = 5
    auto uq, ur = divmod(ua, ub)
    print(uq)  # 3
    print(ur)  # 2

    # divmod by zero -> catchable ZeroDivisionError.
    try:
        auto zq, zr = divmod(1, 0)
        print(zq)
    except ZeroDivisionError:
        print("divzero")  # divzero
