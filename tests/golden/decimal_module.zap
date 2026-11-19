# Golden fixture: the `decimal` module (#981, #981, epic #966) — the exact
# base-10 `Decimal` type (money / billing correctness). Software arbitrary-
# precision decimal over the prelude `bigint` (coefficient + exponent), so it is
# pure prelude Zap with no runtime C. The headline guarantee is exactness:
# `Decimal("0.1") + Decimal("0.2") == Decimal("0.3")` (binary float cannot).
#
# `+ - *` are exact (scale preserved); `/` rounds to a per-call precision with a
# selectable rounding mode (string tags, full GDAS set, default half-even);
# `quantize` sets the scale with rounding. Construction is from str / int only —
# a float argument is a compile error; `decimal.from_float` is the explicit
# opt-in. Every expected value here is cross-checked against CPython's `decimal`.
#
# Documented divergences exercised: no signed zero (a zero result is unsigned),
# no Inf/NaN, Python's `InvalidOperation`/`DivisionByZero` map to `ValueError`.
import decimal

def main():
    # --- the exactness guarantee (the whole reason the type exists) ---
    auto a = decimal.Decimal("0.1")
    auto b = decimal.Decimal("0.2")
    print("exact_sum", str(a + b), str(a + b == decimal.Decimal("0.3")))

    # --- construction + str round-trip (fixed-point and scientific forms) ---
    print("parse_fixed", str(decimal.Decimal("1.50")), str(decimal.Decimal("-0.001")))
    print("parse_int", str(decimal.Decimal(42)), str(decimal.Decimal(-7)))
    print("parse_leadzero", str(decimal.Decimal("000123")), str(decimal.Decimal(".5")))
    print("parse_dot", str(decimal.Decimal("1.")), str(decimal.Decimal("+42")))
    print("parse_ws", str(decimal.Decimal("  3.14  ")))
    print("parse_sci", str(decimal.Decimal("1.5e3")), str(decimal.Decimal("1E-2")))
    print("parse_bigexp", str(decimal.Decimal("9.99E+5")), str(decimal.Decimal("1e10")))
    print("zero_scale", str(decimal.Decimal("0.00")), str(decimal.Decimal("0")))

    # --- big coefficients (arbitrary precision via bigint) ---
    auto big = decimal.Decimal("123456789012345678901234567890.12345")
    print("big_sum", str(big + decimal.Decimal("0.00005")))
    print(
        "big_mul",
        str(
            decimal.Decimal("99999999999999999999") * decimal.Decimal("99999999999999999999"),
        ),
    )

    # --- arithmetic exactness + scale preservation ---
    print("add_scale", str(decimal.Decimal("1.5") + decimal.Decimal("2.25")))
    print("sub", str(decimal.Decimal("5.00") - decimal.Decimal("1.1")))
    print("sub_neg", str(decimal.Decimal("1") - decimal.Decimal("3")))
    print("mul_scale", str(decimal.Decimal("1.50") * decimal.Decimal("1.50")))
    print("mul_zero", str(decimal.Decimal("0") * decimal.Decimal("-3.75")))

    # --- division: exact results reduce to the ideal exponent (like CPython) ---
    auto one = decimal.Decimal(1)
    print("div_exact", str(one / decimal.Decimal(2)), str(one / decimal.Decimal(4)))
    print("div_ten", str(one / decimal.Decimal(10)))
    # --- division: non-terminating, rounded to the requested precision ---
    print("div_third_28", str(one / decimal.Decimal(3)))
    print("div_third_5", str(one.div(decimal.Decimal(3), 5)))
    print("div_seventh_10", str(one.div(decimal.Decimal(7), 10)))
    print("div_neg", str(decimal.Decimal(-5) / decimal.Decimal(3)))
    print(
        "div_rounding",
        str(decimal.Decimal(2).div(decimal.Decimal(3), 1, "ROUND_UP")),
    )

    # --- all eight rounding modes, on a classic half-way tie (quantize to int) ---
    auto tie = decimal.Decimal("2.5")
    auto q1 = decimal.Decimal("1")
    print(
        "round_half_even",
        str(tie.quantize(q1, "ROUND_HALF_EVEN")),
        str(decimal.Decimal("3.5").quantize(q1, "ROUND_HALF_EVEN")),
    )
    print("round_half_up", str(tie.quantize(q1, "ROUND_HALF_UP")))
    print("round_half_down", str(tie.quantize(q1, "ROUND_HALF_DOWN")))
    print("round_down", str(tie.quantize(q1, "ROUND_DOWN")))
    print("round_up", str(tie.quantize(q1, "ROUND_UP")))
    print(
        "round_floor",
        str(decimal.Decimal("-2.5").quantize(q1, "ROUND_FLOOR")),
        str(tie.quantize(q1, "ROUND_FLOOR")),
    )
    print(
        "round_ceiling",
        str(decimal.Decimal("-2.5").quantize(q1, "ROUND_CEILING")),
        str(tie.quantize(q1, "ROUND_CEILING")),
    )
    print(
        "round_05up",
        str(decimal.Decimal("1.5").quantize(q1, "ROUND_05UP")),
        str(decimal.Decimal("2.5").quantize(q1, "ROUND_05UP")),
    )

    # --- quantize to cents (the money operation) + scale up (append zeros) ---
    print("cents_even", str(decimal.Decimal("1.005").quantize(decimal.Decimal("0.01"))))
    print(
        "cents_default",
        str(decimal.Decimal("19.995").quantize(decimal.Decimal("0.01"))),
    )
    print(
        "cents_neg",
        str(
            decimal.Decimal("-1.005").quantize(
                decimal.Decimal("0.01"),
                "ROUND_HALF_UP",
            ),
        ),
    )
    print("scale_up", str(decimal.Decimal("1.5").quantize(decimal.Decimal("0.0001"))))
    print("to_integral", str(decimal.Decimal("2.675").to_integral_value()))

    # --- comparisons (value equality ignores scale: 1.0 == 1.00) ---
    print("eq_scale", str(decimal.Decimal("1.0") == decimal.Decimal("1.00")))
    print(
        "cmp",
        str(decimal.Decimal("0.1") < decimal.Decimal("0.10000001")),
        str(decimal.Decimal("2.5") >= decimal.Decimal("2.5")),
    )
    print("ne", str(decimal.Decimal("1.0") != decimal.Decimal("1.01")))
    print(
        "cmp_neg",
        str(decimal.Decimal("-1") < decimal.Decimal("0")),
        str(decimal.Decimal("-0.5") > decimal.Decimal("-0.6")),
    )

    # --- conversions ---
    print(
        "to_int_trunc",
        str(decimal.Decimal("1.9").to_int()),
        str(decimal.Decimal("-1.9").to_int()),
    )
    print("to_int_big", str(decimal.Decimal("999.999").to_int()))
    print(
        "exponent",
        str(decimal.Decimal("1.500").exponent()),
        str(decimal.Decimal("150").exponent()),
    )
    print(
        "is_zero",
        str(decimal.Decimal("0.00").is_zero()),
        str(decimal.Decimal("0.01").is_zero()),
    )

    # --- explicit float opt-in (shortest round-trip, not exact-binary) ---
    print("from_float", str(decimal.from_float(1.25)), str(decimal.from_float(0.5)))

    # --- the summed-line-items use case: list[Decimal] totals exactly ---
    list[Decimal] cart = [
        decimal.Decimal("9.99"),
        decimal.Decimal("0.01"),
        decimal.Decimal("5.00"),
    ]
    Decimal total = decimal.Decimal("0.00")
    for item in cart:
        total = total + item
    print("cart_total", str(total))

    # --- exception paths (flat model: all map to ValueError) ---
    try:
        auto bad = decimal.Decimal("1.2.3")
        print("unreached")
    except ValueError:
        print("caught_parse")
    try:
        auto dz = decimal.Decimal(1) / decimal.Decimal(0)
        print("unreached")
    except ValueError:
        print("caught_divzero")
    try:
        auto bm = decimal.Decimal("1").quantize(decimal.Decimal("0.1"), "ROUND_BOGUS")
        print("unreached")
    except ValueError:
        print("caught_badmode")
