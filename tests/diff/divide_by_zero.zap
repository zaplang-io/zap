# diff: match
# Integer divide-by-zero raised inside `try` is caught by `except`, exactly like
# CPython's ZeroDivisionError (EXC-3, D-0007 "built-in faults ... try/except
# handles them"). The fault unwinds through Zap's exception machinery
# (`zap_panic`) instead of aborting the process, so the handler and the
# continuation both run. CPython and Zap: body/caught/after, exit 0.
def main():
    int64 z = 0
    try:
        print("body")
        auto q = 1 // z
        print(q)
    except Exception as e:
        print("caught")
    print("after")
