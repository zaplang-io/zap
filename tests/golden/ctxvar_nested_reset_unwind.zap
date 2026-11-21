# Golden fixture (#1237): a nested binding is restored to the previous value on
# BOTH a normal return and an exception unwinding — the restore runs from a
# `finally`, which Zap executes on either exit path.
ContextVar[int64] X = ContextVar[int64]("x", default=0)

def nested(bool boom):
    auto tok = X.set(2)        # shadow the outer binding
    try:
        if boom:
            raise "boom"
    finally:
        X.reset(tok)           # restore on normal AND exceptional exit

def main() -> int64:
    auto t0 = X.set(1)
    print(X.get())             # 1
    nested(False)              # sets 2, restores in finally (normal exit)
    print(X.get())             # 1 — restored
    try:
        nested(True)           # sets 2, raises; finally restores during unwind
    except Exception as e:
        print("caught")
    print(X.get())             # 1 — restored on unwind
    X.reset(t0)
    return 0
