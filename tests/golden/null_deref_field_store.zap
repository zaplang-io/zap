# Golden fixture (#1489 Epic, #1493): a field STORE on a null (non-`Optional`)
# reference raises a catchable, located `NullReferenceError` (the write twin of
# the field-read guard), caught by its own type and by `except Exception`. A
# non-null receiver is written normally. The located message (with line) is
# pinned by `null_deref_field_store_uncaught`; here we assert catch behaviour.
struct Box:
    int64 v

def main() -> int64:
    # Store on a null receiver is caught by its own type.
    Box b = None
    try:
        b.v = 5
        print("WRONG: store did not raise")
    except NullReferenceError:
        print("caught store")

    # ... and by its Exception base.
    Box c = None
    try:
        c.v = 7
    except Exception:
        print("caught store as Exception")

    # A non-null receiver is written and read back normally.
    Box d = Box(v=1)
    d.v = 9
    print(d.v)
    return 0
