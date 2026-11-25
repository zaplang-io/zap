# Golden fixture (#1492, Epic #1489): a bare `raise` (re-raise, #657, D-0116)
# inside a handler re-raises the *same* in-flight exception, preserving its TYPE
# and message. The inner handler catches with the base `Exception` and
# bare-re-raises; the outer handler catches specifically `NullReferenceError`. It
# only matches if the re-raised exception is still a `NullReferenceError` (a base
# `Exception` would not be caught by `except NullReferenceError` and would escape
# uncaught) — so this proves the type (and its located message) survive the
# re-raise.
def main() -> int64:
    try:
        try:
            raise NullReferenceError(
                "null dereference reading field 'head' of 'List' at list.zap:19",
            )
        except Exception:
            print("inner caught, re-raising")
            raise
    except NullReferenceError as e:
        print("outer caught NullReferenceError:")
        print(e)
    return 0
