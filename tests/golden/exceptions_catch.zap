# Golden fixture: raise + try/except/finally with real LLVM stack unwinding
# (#22, D-0007/D-0029). `raise` begins unwinding; control transfers to the
# `except` handler (catch-all for this increment), which prints a recovery
# message; `finally` runs on the way out; execution then continues past the try.
def main() -> int64:
    print("before")
    try:
        print("in try")
        raise "boom"
    except Exception as e:
        print("recovered:")
        print(e)
    finally:
        print("cleanup")
    print("after")
    return 0
