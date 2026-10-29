# claim: EXC-1, EXC-2
# SPEC §10 (#383, D-0007): a built-in fault (here index-out-of-range) raised
# inside `try` is a catchable exception, caught by `except` exactly like an
# explicit `raise` — it unwinds through the exception machinery, not a process
# abort (EXC-1). `finally` runs on the way out and execution continues past the
# block, so the program exits 0 (EXC-2).
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
