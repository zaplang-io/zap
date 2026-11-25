# Golden fixture (#1489 Epic, #1493 part B): a method call on a null built-in
# reference value (a non-`Optional` `list`/`str`/… field left null) raises a
# catchable, located `NullReferenceError` instead of a SIGSEGV — the receiver is
# dereferenced to dispatch. Caught by its own type and by `except Exception`;
# non-null receivers work normally. The located message (with line) is pinned by
# `null_deref_method_uncaught`. (A null USER-struct method receiver is covered
# transitively by the field read/store guards, so it is not tested here.)
struct Bag:
    list[int64] xs = None
    str name = None

def main() -> int64:
    Bag b = Bag()

    # Method on a null built-in list value, caught by its own type.
    try:
        b.xs.append(5)
        print("WRONG: list method did not raise")
    except NullReferenceError:
        print("caught list method")

    # Method on a null str value, caught as its Exception base.
    try:
        auto u = b.name.upper()
        print(u)
    except Exception:
        print("caught str method")

    # Non-null receivers work normally.
    Bag d = Bag(xs=[7], name="hi")
    d.xs.append(8)
    print(len(d.xs))
    print(d.name.upper())
    return 0
