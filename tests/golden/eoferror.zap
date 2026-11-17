# Golden fixture: the EOFError builtin and its place in the hierarchy (ticket
# #1036, D-0215). EOFError is catchable by name, by `except Exception` (it is a
# subclass of Exception, like CPython), and is NOT caught by an unrelated
# `except ValueError`. input()'s end-of-input raise is covered separately by a
# driver test (it needs a piped stdin the golden harness does not provide).
def main():
    try:
        raise EOFError("boom")
    except EOFError:
        print("by-name")

    try:
        raise EOFError("boom")
    except Exception:
        print("by-exception")

    try:
        raise EOFError("boom")
    except ValueError:
        print("wrong")
    except EOFError:
        print("not-valueerror")
