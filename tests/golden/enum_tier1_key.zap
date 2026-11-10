# Golden fixture: a tier-1 (constant) enum is int-backed, so it works as a dict
# key and a set member out of the box under the capability model (D-0139/#809) —
# equality/hashing are intrinsic (backing value, D-0018).
#
# NOTE: `heap[tier-1 enum]` is NOT exercised here. It lowers, but a pre-existing
# runtime defect in the integer-heap element comparator mis-routes an enum element
# (the heap push reaches `zap_str_cmp` and aborts) — unrelated to the enum-as-key
# capability this issue adds. Reported for a separate fix (see PR #817); the
# `heap[Color]` acceptance bullet is deferred to it.
enum Color:
    Red
    Green
    Blue


def main():
    dict[Color, int64] d = {}
    d[Color.Red] = 1
    d[Color.Green] = 2
    print(d[Color.Red])  # 1
    print(d[Color.Green])  # 2
    print(len(d))  # 2

    set[Color] s = set[Color]()
    s.add(Color.Red)
    s.add(Color.Red)  # same member — de-duped
    s.add(Color.Blue)
    print(len(s))  # 2
    print(Color.Blue in s)  # True
