# Golden fixture: a built-in fault (index-out-of-range) raised inside `try` is
# caught by `except` and execution continues (#383, D-0007 "built-in faults ...
# try/except handles them"). The out-of-range access unwinds through the same
# exception machinery as `raise` (`zap_panic`), so the handler binds the
# exception object (the message), `finally` runs, and control falls through past
# the try. Covers both the list and the string index paths.
def main() -> int64:
    list[int64] xs = [10, 20, 30]
    try:
        print("before")
        auto v = xs[9]
        print(v)
    except Exception as e:
        print("caught list")
        print(e)
    finally:
        print("cleanup")
    str s = "hi"
    try:
        auto c = s[5]
        print("unreached")
    except Exception as e:
        print("caught str")
    print("after")
    return 0
