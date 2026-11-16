# Golden fixture: a collection constructor over a type parameter (#983, #1013).
#
# `deque[T]()` / `oset[T]()` inside a generic body must monomorphise to the
# concrete element type per instantiation — the same mechanism that makes
# `list[T]()` work (#1013) — so a single generic helper serves int64 and str.
def seen[T](T a, T b, T c) -> int64:
    auto d = deque[T]()
    d.append(a)
    d.appendleft(b)
    auto s = oset[T]()
    s.add(a)
    s.add(b)
    s.add(c)
    s.add(a)
    return len(d) * 100 + len(s)

def main():
    print(seen[int64](1, 2, 1))
    print(seen[str]("x", "y", "z"))
