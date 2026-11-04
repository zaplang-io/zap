# Golden fixture: the general `wrap(call)` behavioral-decorator form — `retry`
# (D-0036, `design/0004-decorators.md`; #653 of epic #651).
#
# `decorator retry(int64 times)` declares a `wrap(call)` hook whose nested
# `def inner(*args)` replaces the decorated function: it calls the original via
# `call(*args)` inside a `try`, retrying up to `times` times and re-raising on
# the last attempt. Expansion generates — at compile time, before sema, visible
# via `zap expand` — a wrapper with the decorated function's EXACT signature in
# which `call(*args)` is the typed forwarding call `<fn>$orig(<params>)` and the
# decorator argument `times` is bound to a synthetic local.
#
# This exercises: wrapper replacement, `call(*args)` reaching the original,
# argument threading (`x`), return threading (the happy path), the decorator
# argument (`times=3` → three attempts), and exception propagation with a
# re-raise on the final attempt. `*args` is a compile-time forwarding template,
# so the inner body re-raises on the last attempt with `raise e` (a bare `raise`
# is unsupported yet, ZAP6001). `except … as e` binds `e` to the exception's
# message string, and `raise <str>` re-raises as a base `Exception` carrying
# that message (D-0007) — so `main` catches `Exception`.

decorator retry(int64 times):
    def wrap(call):
        def inner(*args):
            for i in range(times):
                try:
                    return call(*args)
                except Exception as e:
                    print("retry attempt", i)
                    if i == times - 1:
                        raise e
        return inner

@retry(times=3)
def always_ok(int64 x) -> int64:
    print("compute", x)
    return x * 2

@retry(times=3)
def always_fail(int64 x) -> int64:
    print("try", x)
    raise ValueError("boom")

def main():
    print("ok result", always_ok(21))
    try:
        print(always_fail(5))
    except Exception as e:
        print("gave up", e)
