# diff: match
# Regression (epic #374, FIXED in #739/B5): a wide fixed-point float format spec
# is now sized dynamically in zap_fmt_flt, so `f"{1e100:f}"` renders the full
# value instead of being silently truncated at ~95 chars. Matches CPython.
def main():
    print(f"{1e100:f}")
    print(f"{1.5:.60f}")
