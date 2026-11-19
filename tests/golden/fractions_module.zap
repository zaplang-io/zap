# Golden fixture: the `fractions` module (#982, epic #966) — the exact rational
# `Fraction` type. A rational over the prelude `bigint` (numerator + positive
# denominator) kept ALWAYS in lowest terms with a normalized sign, so it is pure
# prelude Zap with no runtime C. The headline guarantee is exactness:
# `Fraction(1, 3) + Fraction(1, 3) + Fraction(1, 3) == Fraction(1)` (binary
# float cannot). `+ - * /` are exact; dividing by a zero fraction raises
# `ValueError`. Construction is from two ints, one int, or a str ("3/4" ratio /
# "0.25" decimal) — a float argument is a compile error. Every expected value
# here is cross-checked against CPython's `fractions.Fraction`.
#
# Divergences exercised: no implicit int -> Fraction promotion for operators
# (mix via Fraction(k)); a signed denominator in a ratio literal is rejected
# (like CPython).
import fractions

def main():
    # --- auto-reduction to lowest terms ---
    print("reduce", str(fractions.Fraction(2, 4)), str(fractions.Fraction(100, 25)))

    # --- the exactness guarantee (the whole reason the type exists) ---
    auto third = fractions.Fraction(1, 3)
    auto sum3 = third + third + third
    print("thirds", str(sum3), str(sum3 == fractions.Fraction(1)))

    # --- sign normalization (denominator always > 0) ---
    print("sign", str(fractions.Fraction(1, -2)), str(fractions.Fraction(-3, -6)))

    # --- zero is 0/1 ---
    print(
        "zero",
        str(fractions.Fraction(0, 5)),
        str(fractions.Fraction(0, 5) == fractions.Fraction(0)),
    )

    # --- construction from int / str (ratio + decimal + scientific) ---
    print("from_int", str(fractions.Fraction(7)), str(fractions.Fraction(-8)))
    print("from_str", str(fractions.Fraction("3/4")), str(fractions.Fraction("-5/10")))
    print(
        "from_dec",
        str(fractions.Fraction("0.25")),
        str(fractions.Fraction("-0.001")),
    )
    print(
        "from_sci",
        str(fractions.Fraction("1.5e2")),
        str(fractions.Fraction("2.5E-1")),
    )

    # --- exact arithmetic ---
    auto a = fractions.Fraction(2, 3)
    auto b = fractions.Fraction(3, 4)
    print("add", str(a + b))
    print("sub", str(a - b))
    print("mul", str(a * b))
    print("div", str(a / b))

    # --- mixing with an int (promoted explicitly: no implicit promotion) ---
    print("mix_add", str(fractions.Fraction(1, 2) + fractions.Fraction(3)))
    print("mix_mul", str(fractions.Fraction(2, 5) * fractions.Fraction(10)))

    # --- comparisons: ordering + equality with an int-valued fraction ---
    print("cmp", str(a < b), str(a <= b), str(b > a), str(b >= a))
    print("eq", str(fractions.Fraction(6, 3) == fractions.Fraction(2)), str(a != b))
    print("cmp_neg", str(fractions.Fraction(-1, 2) < fractions.Fraction(0, 1)))

    # --- accessors ---
    auto f = fractions.Fraction(-5, 10)
    print("accessors", str(f.numerator()), str(f.denominator()))

    # --- limit_denominator (continued-fraction convergents) ---
    auto pi = fractions.Fraction(31415926, 10000000)
    print("limden", str(pi.limit_denominator(100)), str(pi.limit_denominator(1000)))
    print("limden_kw", str(pi.limit_denominator(max_denominator=10)))
    print("limden_small", str(fractions.Fraction(3, 4).limit_denominator(100)))

    # --- float conversion (exact for 1/2, shortest round-trip for 1/3) ---
    print(
        "to_float",
        str(fractions.Fraction(1, 2).to_float()),
        str(fractions.Fraction(1, 3).to_float()),
    )

    # --- arbitrary precision (bigint numerator / denominator) ---
    print("big", str(fractions.Fraction("123456789012345678901234567890/987654321")))

    # --- a list of rationals summed exactly ---
    list[Fraction] xs = [
        fractions.Fraction(1, 2),
        fractions.Fraction(1, 3),
        fractions.Fraction(1, 6),
    ]
    Fraction total = fractions.Fraction(0)
    for x in xs:
        total = total + x
    print("list_total", str(total))

    # --- exception paths (flat model: all map to ValueError) ---
    try:
        auto z = fractions.Fraction(1, 0)
        print("unreached")
    except ValueError:
        print("caught_zeroden")
    try:
        auto dz = fractions.Fraction(1, 2) / fractions.Fraction(0)
        print("unreached")
    except ValueError:
        print("caught_divzero")
    try:
        auto bad = fractions.Fraction("1.2.3")
        print("unreached")
    except ValueError:
        print("caught_parse")
    try:
        auto bad2 = fractions.Fraction("1/-2")
        print("unreached")
    except ValueError:
        print("caught_signden")
