# Golden fixture: integer and unsigned subscript indices (#552 follow-up).
#
# A subscript index of any integer width auto-promotes to the 64-bit index word:
# a signed index sign-extends (so a negative index is from-the-end, Python
# `[-1]`), an unsigned index zero-extends (always a non-negative offset, so a
# high byte like 200 is never mistaken for a negative index). Covers `list` and
# `array[uint8]` element get *and* set, across every small integer width and
# `usize`. Values are chosen to format exactly.

def main():
    list[int64] xs = [10, 20, 30, 40, 50]

    int32 a = 2
    xs[a] = 99  # int32 index, store
    print(xs[a])  # 99

    uint8 b = 1
    print(xs[b])  # 20   uint8 index (zero-extended, never negative)

    uint16 c = 4
    print(xs[c])  # 50

    uint32 d = 3
    print(xs[d])  # 40

    int32 neg = -1
    print(xs[neg])  # 50   negative signed index = from the end (unchanged)

    usize n = 0
    print(xs[n])  # 10   usize index

    # An unsigned index zero-extends, so it is never read as from-the-end. A
    # `uint64`/`usize` above INT64_MAX reinterprets to a negative int64 with the
    # *same bits* as a signed negative index, but must NOT wrap from the end: it
    # exceeds any real length, so it raises a catchable IndexError. `uint64(-1)`
    # is bit-identical to the int64 `-1` of `neg` above, yet behaves oppositely.
    uint64 huge = uint64(-1)  # 2**64 - 1
    try:
        print(xs[huge])  # not xs[-1]; overflow is clamped out of range
    except Exception as e:
        print(e)  # list index 9223372036854775807 out of range (len 5)

    # A small uint64 index is unaffected — a normal non-negative offset.
    uint64 small = 3
    print(xs[small])  # 40

    # array[uint8] (dense bytes) indexed and stored through unsigned/signed indices.
    array[uint8] buf = array[uint8]([5, 6, 7, 8])
    uint16 i = 2
    print(buf[i])  # 7
    int32 j = 1
    buf[j] = 200
    print(buf[j])  # 200
