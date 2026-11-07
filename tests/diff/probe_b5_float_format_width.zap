# diff: match
# PROBE (epic #374, suspected bug B5): `zap_fmt_flt` formats into a fixed 96-byte
# buffer, so a wide fixed-point spec is suspected to be silently truncated
# (snprintf bounds the write, so it is wrong text, not a memory fault). The
# MATCH-FAILED report shows Zap's actual (possibly truncated) output. CPython
# prints a 1-followed-by-100-zeros integer part with `.000000`, then `1.5`
# padded to 60 fractional digits:
#   10000000000000000159028911097599180468360808563945281389781327557747838772170381060813469985856815104.000000
#   1.500000000000000000000000000000000000000000000000000000000000
def main():
    print(f"{1e100:f}")
    print(f"{1.5:.60f}")
