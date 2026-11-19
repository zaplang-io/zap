# Golden fixture: the catchable `ValueError` faults of `bytes.translate` /
# `bytes.maketrans` (ticket #1099). A `translate` table that is not exactly 256
# bytes (both shorter and longer) and a `maketrans` whose `frm`/`to` differ in
# length each raise a catchable `ValueError` (CPython parity), reaching a
# `try/except` rather than terminating the process.
def main():
    # table shorter than 256 bytes -> ValueError
    try:
        print(str(b"abc".translate(b"short")))
    except ValueError:
        print("short-table-caught")

    # table longer than 256 bytes -> ValueError. Build a 300-byte table.
    auto ident = bytes.maketrans(b"", b"")
    auto toolong = ident + bytes([0, 0, 0, 0])  # 260 bytes
    try:
        print(str(b"abc".translate(toolong)))
    except ValueError:
        print("long-table-caught")

    # maketrans with unequal frm/to lengths -> ValueError
    try:
        auto t = bytes.maketrans(b"abc", b"xy")
        print(str(t))
    except ValueError:
        print("maketrans-len-caught")

    # a valid table still works after the caught faults (state is intact).
    auto ok = bytes.maketrans(b"a", b"Z")
    print(str(b"banana".translate(ok)))  # bZnZnZ
