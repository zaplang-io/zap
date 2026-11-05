# Golden fixture: a bare `raise` (#657) re-raises the *innermost* active
# exception. Here the outer handler is handling `ValueError("outer")`; inside
# it, a nested `try` raises and catches `RuntimeError("inner")`, and a bare
# `raise` in the nested handler re-raises *that* inner exception (not the outer
# one). With no further handler it escapes `main` uncaught, so the process
# exits non-zero and stderr names the inner exception's message. The harness
# checks the exit code (.exit) and the stderr substring (.stderr).
def main() -> int64:
    try:
        raise ValueError("outer")
    except Exception:
        print("outer handler")
        try:
            raise RuntimeError("inner")
        except Exception:
            print("inner handler")
            raise
    return 0
