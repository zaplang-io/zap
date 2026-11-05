# Golden fixture: `except E as e:` binds the caught exception, and a following
# bare `raise` (#657) re-raises that same exception object — the binding and the
# re-raise observe one and the same exception (type + message preserved). The
# inner handler binds and prints it, then bare-re-raises; the outer handler,
# matching the original `KeyError`, prints it again. Identical messages from
# both handlers show it is the same object flowing through.
def main() -> int64:
    try:
        try:
            raise KeyError("missing")
        except Exception as e:
            print("inner caught:")
            print(e)
            raise
    except KeyError as e2:
        print("outer re-caught KeyError:")
        print(e2)
    return 0
