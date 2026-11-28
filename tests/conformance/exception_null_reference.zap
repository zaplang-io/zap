# claim: EXC-8
# SPEC §10 (#1489, decisions/lang/1489-null-reference-error.md): dereferencing a
# null (non-`Optional`) reference raises the dedicated, catchable
# `NullReferenceError` across the whole surface — a field read, a field store, a
# method-call receiver, an index base, and a for-iteration base — caught by its
# own name and by `except Exception`, so the footgun is debuggable and
# recoverable instead of a SIGSEGV (EXC-8). The located message *shape* is pinned
# by the goldens (`none_access_uncaught` et al.); here we pin the type and its
# catch behaviour across every site the guard covers.
struct Point:
    int64 x

    def get(self) -> int64:
        return self.x

struct Bag:
    list[int64] items = None

def main():
    # Field read on a null receiver — caught by its own name.
    Point p = None
    try:
        auto v = p.x
        print(v)
    except NullReferenceError:
        print("field read")

    # Field store on a null receiver — caught as the `Exception` base.
    Point q = None
    try:
        q.x = 1
    except Exception:
        print("field store")

    # Method-call receiver on a null user struct — caught by its own name.
    Point r = None
    try:
        auto w = r.get()
        print(w)
    except NullReferenceError:
        print("method receiver")

    # Index base on a null built-in `list` — caught by its own name.
    Bag b = Bag()
    try:
        auto e = b.items[0]
        print(e)
    except NullReferenceError:
        print("index base")

    # For-iteration base on a null built-in `list` — caught as the base.
    Bag c = Bag()
    try:
        for y in c.items:
            print(y)
    except Exception:
        print("iter base")
