# Borrowed-bytes bytesview (#1082 follow-up): the `bytes` counterpart of strview.
# A non-owning view over an existing `bytes` value's storage — zero alloc, zero
# copy — created with b.view()/b.view(lo, hi). Bounds are BYTE indices (I#337).
# The read-only probes / == / hash / dict lookup reuse strview's raw-byte runtime;
# only creation and `len` (byte count) are byte-specific. Only lookups and counts
# are printed, so the golden is independent of hash iteration order.
def main():
    bytes b = bytes([104, 101, 108, 108, 111])
    bytesview v = b.view(0, 2)
    print(len(v))  # 2
    print(v.startswith(bytes([104])))  # True
    print(v.endswith(bytes([101])))  # True
    print(v.contains(bytes([101])))  # True
    print(v == bytes([104, 101]))  # True
    print(v == bytes([104]))  # False
    bytesview w = b.view(2)  # to end
    print(len(w))  # 3
    bytesview sub = w.view(0, 2)  # sub-view of a view
    print(len(sub))  # 2
    bytes o = v.to_bytes()  # materialize owned bytes
    print(len(o))  # 2

    # bytes-keyed dict probe with a borrow (reuses the str-key entry points).
    dict[bytes, int64] d = {}
    d[bytes([104, 101])] = 7
    print(v in d)  # True
    print(d.get(v, -1))  # 7
