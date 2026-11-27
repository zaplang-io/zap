# Golden fixture: str -> bigint and str -> complex scalar conversions (#1657).
#
# `bigint(str)` parses an arbitrary-precision base-N integer (beyond int64);
# `complex(str)` parses a Python-style complex literal. A malformed string raises
# a catchable ValueError, like int(str)/float(str)
# (decisions/lang/1657-str-parse-conversions.md).

def main():
    bigint n = bigint("123456789012345678901234567890")
    print(n)
    complex a = complex("1+2j")
    print(a)
    complex b = complex("3j")
    print(b)
    complex c = complex("-1.5-2e3j")
    print(c)
    # A malformed string raises a catchable ValueError.
    try:
        auto bad = bigint("nope")
        print(bad)
    except ValueError:
        print("bad-bigint")
    try:
        auto badc = complex("1+j")
        print(badc)
    except ValueError:
        print("bad-complex")
