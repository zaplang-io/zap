# Golden (epic #374, B2): integer `**` with a negative exponent raises a
# catchable ValueError rather than silently flooring to 0. Type-honest —
# `int ** int` stays `int`; use a float base (`2.0 ** -1`) for a real negative
# power. CPython returns a float here; see
# tests/diff/probe_b2_int_pow_negative.zap.
def main():
    int64 e = -1
    try:
        print(2 ** e)
    except ValueError as ex:
        print("caught")
