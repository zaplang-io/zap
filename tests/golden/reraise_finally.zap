# Golden fixture: a bare `raise` (#657) inside an `except` handler still runs
# the enclosing `try`'s `finally` on the way out, then the re-raised exception
# propagates to the outer handler. Ordering: the handler prints, the bare
# `raise` re-throws the active exception, `finally` runs during the unwind, and
# the outer `except` finally recovers it (message preserved).
def main() -> int64:
    try:
        try:
            raise RuntimeError("fail")
        except Exception:
            print("handling")
            raise
        finally:
            print("finally ran")
    except Exception as e:
        print("recovered:")
        print(e)
    return 0
