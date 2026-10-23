# Golden fixture: value structs with operators and as a dict/set key (#65,
# D-0051). Operator overloading and @derive (D-0047) work identically on a value
# struct. Stored in a dict/set, a value struct is BOXED — copied to the heap — so
# the stored key is independent of the original: mutating the original after
# insertion does not disturb the entry.
@derive(Eq, Hash, Repr)
value struct Point:
    int64 x
    int64 y

def main():
    auto p = Point(x=1, y=2)
    auto q = Point(x=1, y=2)
    if p == q:  # derived __eq__ on a value struct
        print("p == q")
    print(p)  # derived __repr__

    auto d = {p: 100}
    p.x = 99  # mutate the ORIGINAL after insertion
    auto orig = Point(x=1, y=2)
    print(d[orig])  # 100 — the boxed key copy is unaffected by p.x=99

    # Two equal value structs collapse to a single set element (hashed/compared
    # via the derived dunders).
    auto s = {q, orig}
    print(len(s))
