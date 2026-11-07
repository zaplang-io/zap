# claim: NUM-8
# SPEC §5 (#374, #751): float `%` is floored — it takes the sign of the divisor,
# consistent with floored float `//`, so `a == (a // b) * b + (a % b)` holds for
# floats as for ints. A zero remainder takes the divisor's sign (`5.0 % -5.0` is
# -0.0). Modulo by zero raises a catchable ZeroDivisionError.
def main():
    print(str(5.5 % -2))
    print(str(-5.5 % 2))
    print(str(-7.0 % 3.0))
    print(str(5.0 % -5.0))
    try:
        print(str(1.0 % 0.0))
    except ZeroDivisionError:
        print("zero")
