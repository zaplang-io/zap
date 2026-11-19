# Golden fixture: len() on a `bytes` element read back from a collection counts
# BYTES, not UTF-8 codepoints (#1074, D-0241).
#
# A `bytes` value stored in a `list`/`dict`/`set` and read back — via indexing
# (`rows[i]`), iteration (`for row in rows`), a dict value (`m[key]`), or a set
# member — must report the same byte length as the direct `bytes` binding. The
# regression collapsed a `bytes` element to a bare `str` pointer, so read-back
# took the `str` codepoint count (`zap_str_len_cp`) instead of the byte count
# (`zap_str_len`): a 5-byte / 4-codepoint payload reported 4.
#
# Byte-set coverage: empty, pure ASCII, multibyte UTF-8, an embedded zero byte
# (0x00), and arbitrary non-UTF-8 bytes (0xFF, 0x80). List iteration order is
# defined, so the loop output is asserted in order; `set` iteration order is
# unspecified (D-0235), so each set case holds a single element.
def main():
    # Each payload pairs its raw byte count with its UTF-8 codepoint count — the
    # codepoint count is exactly what the #1074 bug reported for a list element.
    bytes empty = bytes()  # 0 bytes
    bytes ascii = bytes([104, 105])  # 2 bytes ("hi"), 2 codepoints
    bytes utf8 = bytes([99, 97, 102, 195, 169])  # 5 bytes ("café"), 4 codepoints
    bytes zero = bytes([195, 169, 0, 255, 128])  # 5 bytes, 4 codepoints (issue payload)
    bytes raw = bytes([255, 128])  # 2 bytes, arbitrary non-UTF-8

    # Direct bindings are already correct — the reference byte counts.
    print(len(empty))  # 0
    print(len(ascii))  # 2
    print(len(utf8))  # 5
    print(len(zero))  # 5
    print(len(raw))  # 2

    list[bytes] rows = [empty, ascii, utf8, zero, raw]

    # Indexed read-back: `rows[i]` must match the direct byte count.
    print(len(rows[0]))  # 0
    print(len(rows[1]))  # 2
    print(len(rows[2]))  # 5
    print(len(rows[3]))  # 5
    print(len(rows[4]))  # 2

    # Iterated read-back: `for row in rows` (list order is defined).
    for row in rows:
        print(len(row))  # 0, 2, 5, 5, 2

    # `bytes` as a dict VALUE: read back via `m[key]`.
    dict[str, bytes] m = {"zero": zero}
    print(len(m["zero"]))  # 5

    # `bytes` as a set MEMBER: a single-element set keeps the output
    # order-independent (set iteration order is unspecified, D-0235).
    set[bytes] s = {zero}
    for item in s:
        print(len(item))  # 5
