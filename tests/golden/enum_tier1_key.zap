# Golden fixture: a tier-1 (constant) enum is int-backed, so it works as a dict
# key, a set member, and a heap element out of the box under the capability model
# (D-0139/#809) — equality/hashing are intrinsic (backing value, D-0018) and the
# heap orders by that backing int.
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

    # Min-heap by backing int (Red=0 < Blue=2), so the smallest pops first.
    heap[Color] h = heap[Color]()
    h.push(Color.Blue)
    h.push(Color.Red)
    print(h.pop() == Color.Red)  # True
