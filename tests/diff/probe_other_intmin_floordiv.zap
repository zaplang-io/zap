# diff: diverge #742: INT64_MIN // -1 wraps to INT64_MIN (wrapping int64, D-0019); CPython uses bigint. FIXED the process abort in #742 — now a defined value, not a crash.
# PROBE (epic #374, §10 "other"): INT64_MIN // -1 and INT64_MIN % -1. On x86 the
# `idiv` instruction traps on MIN/-1. Zap now defines these (D-0019 wrapping
# int64): INT64_MIN // -1 == INT64_MIN and INT64_MIN % -1 == 0, no process abort
# (see tests/golden/intmin_floordiv_wrap.zap). CPython has arbitrary-width ints,
# so it computes the exact results instead and prints:
#   -9223372036854775808
#   9223372036854775808
#   0
def main():
    int64 lo = -9223372036854775807 - 1
    print(lo)
    print(lo // -1)
    print(lo % -1)
