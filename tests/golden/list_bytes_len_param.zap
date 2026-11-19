# Golden fixture: the exact #1074 reproduction — a `list[bytes]` passed to a
# function, read back by indexing and by iteration (D-0241).
#
# This is the issue's minimal repro: the `bytes` element reaches `check` through
# a `list[bytes]` parameter, so the fix must survive the function-parameter path
# (the element kind travels in the parameter's `Ty::List(Elem::Bytes)`), not
# only a local list. Expected output is `5` for the direct binding and for both
# the indexed and the iterated read-back; the pre-fix bug printed `5` then `4`.
def check(list[bytes] rows):
    print(len(rows[0]))  # 5  (indexed read-back through the parameter)
    for row in rows:
        print(len(row))  # 5  (iterated read-back through the parameter)


def main():
    bytes sample = bytes([195, 169, 0, 255, 128])  # 5 bytes, 4 codepoints
    print(len(sample))  # 5  (direct binding)
    list[bytes] rows = [sample]
    check(rows)
