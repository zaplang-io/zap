# Golden fixture (#1237): `get()` returns the construction default when the
# variable is unbound in this coroutine; a variable with NO default raises a
# catchable LookupError — proving the raising runtime call is lowered as an
# `invoke` so `try/except` catches it (not a process abort).
ContextVar[int64] WITH_DEFAULT = ContextVar[int64]("wd", default=5)
ContextVar[int64] NO_DEFAULT = ContextVar[int64]("nd")

def main() -> int64:
    print(WITH_DEFAULT.get())  # 5 — default, never set
    try:
        auto x = NO_DEFAULT.get()
        print(x)  # unreached
    except Exception as e:
        print("caught")
    return 0
