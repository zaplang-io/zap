# Golden fixture: operations that take a `bytes` value directly (ticket #1355),
# building on byte iteration (#1349).
#
# `sum(bytes)` reduces the byte values to an int64 (optional start); `sorted(bytes)`
# returns a fresh list[int64] of the byte values ascending (reverse= supported);
# `x in bytes` is membership — an int left operand is byte-value membership
# (0..=255), a bytes left operand is subsequence membership (Python semantics,
# empty subsequence is always present). `not in` negates.
def main():
    # sum: 85 + 84 + 67 == 236; with a start; empty bytes is the identity.
    print(sum(b"UTC"))  # 236
    print(sum(b"UTC", 4))  # 240
    print(sum(bytes()))  # 0

    # sorted: ascending byte values as a list[int64] (c=99, a=97, b=98).
    auto asc = sorted(b"cab")
    print(asc[0])  # 97
    print(asc[2])  # 99
    print(asc == [97, 98, 99])  # True
    auto desc = sorted(b"cab", reverse=True)
    print(desc[0])  # 99
    print(desc == [99, 98, 97])  # True
    print(len(sorted(bytes())))  # 0

    # int membership: byte-value present / absent (unsigned, so 255 is findable).
    print(85 in b"UTC")  # True   (U)
    print(0 in b"UTC")  # False
    print(0 not in b"UTC")  # True
    auto edges = bytes([0, 255])
    print(255 in edges)  # True
    print(1 in edges)  # False

    # bytes membership: subsequence present / absent; empty is always present.
    print(b"TC" in b"UTC")  # True
    print(b"UTC" in b"UTC")  # True
    print(b"xy" in b"UTC")  # False
    print(b"" in b"UTC")  # True
    print(b"TC" not in b"UTC")  # False
