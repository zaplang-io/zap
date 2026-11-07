# diff: diverge BUG #742: INT64_MIN // -1 and % -1 abort the process (idiv trap) — CONFIRMED by PR #738 CI
# PROBE (epic #374, §10 "other"): INT64_MIN // -1 and INT64_MIN % -1. On x86 the
# `idiv` instruction traps on MIN/-1, so the question is whether Zap turns that
# into a catchable fault or crashes the process. INT64_MIN is written as
# `-9223372036854775807 - 1` because the bare literal 9223372036854775808 is
# above int64 and would promote to bigint (D-0019). CPython has arbitrary-width
# ints, so it just computes the exact results (no trap) and prints:
#   -9223372036854775808
#   9223372036854775808
#   0
def main():
    int64 lo = -9223372036854775807 - 1
    print(lo)
    try:
        print(lo // -1)
    except Exception as e:
        print("caught-div")
    try:
        print(lo % -1)
    except Exception as e:
        print("caught-mod")
