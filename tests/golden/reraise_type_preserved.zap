# Golden fixture: a bare `raise` (re-raise, #657, D-0116) inside an `except`
# handler re-raises the *same* in-flight exception, preserving its TYPE. The
# inner handler catches with the base `Exception` and bare-re-raises; the outer
# handler catches specifically `ValueError`. It only matches if the re-raised
# exception is still a `ValueError` (a base `Exception` would not be caught by
# `except ValueError` and would escape uncaught) — so this proves the type
# (and message) survive the re-raise.
def main() -> int64:
    try:
        try:
            raise ValueError("boom")
        except Exception:
            print("inner caught, re-raising")
            raise
    except ValueError as e:
        print("outer caught ValueError:")
        print(e)
    return 0
