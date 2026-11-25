# Golden fixture (#1489 Epic, #1493): a field READ on a null (non-`Optional`)
# reference raises a catchable, located `NullReferenceError` (#1492) instead of a
# SIGSEGV. It is caught by its own name and by `except Exception`, but NOT by an
# unrelated clause; `finally` still runs; a non-null receiver reads normally. The
# located message itself (with the source line) is pinned by the matching
# `none_access_uncaught` stderr golden — here we assert catch behaviour, which is
# deterministic regardless of the build path's file name.
struct Point:
    int64 x
    int64 y

def main() -> int64:
    # Caught by its own type; `finally` runs; control falls through.
    Point p = None
    try:
        print("before")
        auto v = p.x
        print(v)
    except NullReferenceError:
        print("caught NullReferenceError")
    finally:
        print("cleanup")

    # Also catchable as its `Exception` base (flat placement, #1492).
    Point q = None
    try:
        auto w = q.x
        print(w)
    except Exception:
        print("caught as Exception")

    # NOT caught by an unrelated clause: the inner `except ValueError` is skipped
    # and the null deref propagates to the enclosing `except NullReferenceError`.
    Point r = None
    try:
        try:
            auto z = r.x
            print(z)
        except ValueError:
            print("WRONG: valueerror caught null deref")
    except NullReferenceError:
        print("propagated past ValueError")

    # A non-null receiver is read normally.
    Point s = Point(x=3, y=4)
    print(s.x)
    print("after")
    return 0
