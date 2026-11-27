# diff: match
#
# bytearray parity (#1385): construction, indexing (-> int), append, extend, +=,
# `a + b`, len, equality against bytes/bytearray, and byte-wise iteration all
# match CPython's `bytearray`. str(bytes) is deliberately NOT exercised here —
# Zap's str(bytes) decodes UTF-8 while CPython returns the repr (a documented
# divergence covered elsewhere), so this case prints only ints and booleans.
def main():
    auto a = bytearray(b"abc")
    print(len(a))  # 3
    print(a[0])  # 97
    print(a[2])  # 99
    print(a[-1])  # 99
    a[0] = 65
    print(a[0])  # 65
    a.append(100)
    a.extend(b"ef")
    a.extend([1, 2])
    print(len(a))  # 8
    a += b"gh"
    a += [3]
    print(len(a))  # 11
    auto total = 0
    for by in a:
        total = total + by
    print(total)  # 778
    auto p = bytearray(b"foo")
    auto q = bytearray(b"bar")
    auto r = p + q
    print(len(r))  # 6
    print(r[0])  # 102
    print(r[3])  # 98
    print(str(p == bytearray(b"foo")))  # True
    print(str(p == b"foo"))  # True
    print(str(p == q))  # False
    auto z = bytearray(4)
    print(len(z))  # 4
    print(z[0])  # 0
    print(len(bytearray()))  # 0
    auto fl = bytearray([10, 20, 30])
    print(fl[1])  # 20
