# Golden fixture: integer divide-by-zero raised inside `try` is caught by
# `except` and execution continues (EXC-3, D-0007). Both `//` and `%` by zero
# unwind through the same exception machinery as `raise` (`zap_panic`), so the
# handler binds the exception object (the message), `finally` runs, and control
# falls through past the try.
def main() -> int64:
    int64 z = 0
    try:
        print("before")
        auto q = 7 // z
        print(q)
    except Exception as e:
        print("caught div")
        print(e)
    finally:
        print("cleanup")
    try:
        auto r = 7 % z
        print("unreached")
    except Exception as e:
        print("caught mod")
    print("after")
    return 0
