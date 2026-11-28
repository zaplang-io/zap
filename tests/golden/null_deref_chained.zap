# Golden fixture (#1489 Epic, #1497): the null-deref guard raises the same
# catchable `NullReferenceError` when the null reference is reached through a
# *composition*, not only as a bare `local.field`. Three compositional shapes
# are walled in: a chained access `o.inner.v` whose intermediate `o.inner` is
# the default null, a null deref evaluated as a call argument (the guard fires
# while the argument is built, before the call), and a null deref inside a loop
# body (the guard fires on the first iteration and unwinds out of the loop).
# Each is caught and execution continues; once the intermediate is non-null the
# whole chain reads normally.
struct Inner:
    int64 v

struct Outer:
    Inner inner = None

def take(int64 n) -> int64:
    return n

def main() -> int64:
    # Chained access: `o.inner` is the default null, so reading `.v` through it
    # dereferences a null intermediate.
    Outer o = Outer()
    try:
        auto a = o.inner.v
        print(a)
    except NullReferenceError:
        print("caught chained")

    # Null deref as a call argument: caught as the Exception base.
    Outer p = Outer()
    try:
        auto r = take(p.inner.v)
        print(r)
    except Exception:
        print("caught in argument")

    # Null deref inside a loop body: unwinds out of the loop, caught outside.
    Outer q = Outer()
    try:
        for i in range(3):
            print(q.inner.v)
    except NullReferenceError:
        print("caught in loop")

    # A non-null intermediate reads the whole chain normally.
    Outer d = Outer(inner=Inner(v=42))
    print(d.inner.v)
    return 0
