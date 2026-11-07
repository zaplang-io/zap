# diff: diverge #740: shift count >= 64 is now a DEFINED "shifted out" value (1 << 64 -> 0, -1 >> 100 -> -1), no longer LLVM poison; CPython uses bigint so 1 << 64 still differs. FIXED the UB in #740.
# PROBE (epic #374, #740): shift counts >= the type width. Zap's int is a
# wrapping int64, so a count >= 64 shifts every bit out — defined as 0 for `<<`
# and as the sign bit for arithmetic `>>` (zap_ishl/zap_ishr) — rather than the
# previous unmasked-shift poison. CPython has arbitrary-width ints and prints:
#   18446744073709551616
#   -1
def main():
    print(1 << 64)
    print(-1 >> 100)
