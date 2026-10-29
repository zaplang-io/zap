# diff: diverge BUG: a built-in fault (here index-out-of-range) is NOT caught by
# `except` — Zap unwinds past the handler and aborts the process ("list index 5
# out of range", exit 1), so the `finally` and the continuation never run.
# CPython catches it and runs except + finally normally. D-0007 says "built-in
# faults ... try/except handles them", so this is a real bug to triage. (An
# explicitly `raise`d value IS caught — tests/golden/exceptions_catch.zap — but
# that form is not Python-compatible, so this built-in-fault form is the shared
# one.) CPython: body/caught/fin/after, exit 0; Zap: body then abort.
def main():
    list[int64] data = [1, 2]
    try:
        print("body")
        auto v = data[5]
        print(v)
    except Exception as e:
        print("caught")
    finally:
        print("fin")
    print("after")
