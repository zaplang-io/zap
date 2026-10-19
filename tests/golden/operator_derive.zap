# Golden fixture: @derive(Eq, Ord, Hash, Repr) + struct dict/set keys (#52,
# D-0047). The derive generates `__eq__`/`__cmp__`/`__hash__`/`__repr__`
# field-wise, so `==`/`<` work, `print(p)` renders via `__repr__`, and the struct
# is usable as a `set`/`dict` key (the D-0043 struct-element-hashing unblock).
# The `a == b => hash(a) == hash(b)` invariant holds because the same fields feed
# `__eq__` and `__hash__`.
# The `str` field exercises content-based hashing/equality/ordering: `q`'s "a"
# is a distinct string object from `p`'s "a", but they hash and compare equal, so
# `q` hits `p`'s dict/set entry (the invariant `a == b => hash(a) == hash(b)`).
@derive(Eq, Ord, Hash, Repr)
struct Point:
    int64 x
    str tag

def main():
    auto p = Point(x=1, tag="a")
    auto q = Point(x=1, tag="a")
    auto r = Point(x=1, tag="b")
    if p == q:
        print("p == q")
    if p < r:
        print("p < r")
    print(p)
    # A set of struct keys, hashed/compared via the derived dunders.
    auto s = {p, r}
    print(len(s))
    if q in s:
        print("q in s")
    auto t = Point(x=9, tag="z")
    if t in s:
        print("t in s")
    else:
        print("t not in s")
    # A dict keyed by structs: `q` hits `p`'s entry (q == p, same hash).
    auto d = {p: 100, r: 200}
    print(d[q])
    print(d[r])
