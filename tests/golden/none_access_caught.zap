# Golden fixture: member access on a `None` reference struct raised inside `try`
# is caught by `except` and execution continues (EXC-4, D-0007/D-0014). Reading a
# field of a `None` receiver unwinds through the same exception machinery as
# `raise` (`zap_panic`), so the handler binds the exception object (the message),
# `finally` runs, and control falls through past the try. A non-null receiver is
# read normally in the same program.
struct Point:
    int64 x
    int64 y

def main() -> int64:
    Point p = None
    try:
        print("before")
        auto v = p.x
        print(v)
    except Exception as e:
        print("caught")
        print(e)
    finally:
        print("cleanup")
    Point q = Point(x=3, y=4)
    print(q.x)
    print("after")
    return 0
