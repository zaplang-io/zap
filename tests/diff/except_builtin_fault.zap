# diff: match
# A built-in fault (here index-out-of-range) raised inside `try` is caught by
# `except`, exactly like CPython's IndexError (#383, D-0007 "built-in faults ...
# try/except handles them"). The fault now unwinds through Zap's exception
# machinery (`zap_panic`) instead of aborting the process, so the handler, the
# `finally`, and the continuation all run. CPython and Zap: body/caught/fin/after,
# exit 0.
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
