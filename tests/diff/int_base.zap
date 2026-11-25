# diff: match
# int(str, base) matches CPython (#1458): bases 2..36, base-0 prefix auto-detect
# (0x/0o/0b), an explicit prefix accepted for the matching base, surrounding
# whitespace, and a leading sign. Error cases (bad digit/base) are asserted in
# the golden suite (Zap returns int64, CPython an arbitrary-precision int, so the
# diff corpus keeps to in-range values).
def main():
    print(int("ff", 16))
    print(int("0b1010", 0))
    print(int("0o777", 0))
    print(int("0xFF", 0))
    print(int("777", 8))
    print(int("z", 36))
    print(int("-10", 2))
    print(int("42", 0))
    print(int("  101  ", 2))
    print(int("0xFF", 16))
    print(int("+7F", 16))
