# Golden fixture: the `bytearray` mutable byte buffer (#1385).
#
# Constructors (bytes, int length, empty, list[int], bytearray copy, str), indexed
# get/set (-> int64 in 0..=255), append / extend (bytes|bytearray|list[int]), += and
# `a + b` (a NEW bytearray), `bytes(ba)` round-trip, len, equality vs bytes and
# bytearray by content, byte-wise iteration (ints), and a slice read.
def main():
    # bytearray(bytes): a mutable copy.
    auto a = bytearray(b"abc")
    print(len(a))  # 3
    print(a[0])  # 97
    print(a[-1])  # 99

    # indexed set, in place.
    a[0] = 65
    print(a[0])  # 65
    print(str(bytes(a)))  # Abc

    # append one byte; extend with bytes / bytearray / list[int].
    a.append(100)  # d
    a.extend(b"ef")
    auto other = bytearray([103, 104])  # gh
    a.extend(other)
    a.extend([105, 106])  # ij
    print(str(bytes(a)))  # Abcdefghij
    print(len(a))  # 10

    # += (in-place extend): bytes, bytearray, list[int].
    a += b"KL"
    a += other  # gh
    a += [77]  # M
    print(str(bytes(a)))  # AbcdefghijKLghM
    print(len(a))  # 15

    # a + b -> a NEW bytearray; the operands are unchanged.
    auto p = bytearray(b"foo")
    auto q = bytearray(b"bar")
    auto r = p + q
    print(str(bytes(r)))  # foobar
    print(str(bytes(p)))  # foo  (unchanged)
    auto r2 = p + b"XYZ"  # bytearray + bytes
    print(str(bytes(r2)))  # fooXYZ

    # zero-filled, empty, from list[int], from str.
    auto z = bytearray(4)
    print(len(z))  # 4
    print(z[0])  # 0
    auto e = bytearray()
    print(len(e))  # 0
    auto fl = bytearray([120, 121, 122])
    print(str(bytes(fl)))  # xyz
    auto fs = bytearray("hi")
    print(str(bytes(fs)))  # hi

    # equality vs bytes and bytearray, by content.
    print(str(p == b"foo"))  # True
    print(str(p == bytearray(b"foo")))  # True
    print(str(p == q))  # False
    print(str(p != q))  # True

    # byte-wise iteration yields int64 0..255.
    auto s = bytearray(b"ABC")
    int64 total = 0
    for by in s:
        total = total + by
    print(total)  # 198

    # slice read -> a fresh bytearray.
    auto sub = s[1:3]
    print(str(bytes(sub)))  # BC
    print(len(sub))  # 2
