# Golden (epic #374, issue #742): INT64_MIN // -1 and INT64_MIN % -1 are defined
# under Zap's wrapping int64 (D-0019) and do NOT abort the process. The hardware
# `idiv` traps on MIN/-1, so zap_ifloordiv/zap_imod special-case it: the quotient
# wraps to INT64_MIN and the remainder is 0. CPython differs (arbitrary-width
# int); see tests/diff/probe_other_intmin_floordiv.zap.
def main():
    int64 lo = -9223372036854775807 - 1
    print(lo)
    print(lo // -1)
    print(lo % -1)
