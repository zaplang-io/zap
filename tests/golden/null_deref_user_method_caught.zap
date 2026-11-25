# Golden fixture (#1489 Epic, #1541): a method call on a null USER reference-struct
# receiver raises a catchable, located `NullReferenceError` at the call site,
# caught by its own type and by `except Exception`. A non-null receiver works
# normally. The located message (with line) is pinned by
# `null_deref_user_method_uncaught`.
struct Node:
    int64 v

    def inc(self):
        self.v = self.v + 1

    def get(self) -> int64:
        return self.v

def main() -> int64:
    Node n = None
    try:
        n.inc()
        print("WRONG: method did not raise")
    except NullReferenceError:
        print("caught inc")

    Node m = None
    try:
        auto v = m.get()
        print(v)
    except Exception:
        print("caught get")

    Node d = Node(v=5)
    d.inc()
    print(d.get())
    return 0
