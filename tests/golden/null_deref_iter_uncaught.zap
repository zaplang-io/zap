# Golden fixture (#1489 Epic, #1497): an *uncaught* for-iteration over a null
# (non-`Optional`) `list` reference reports the located `NullReferenceError`
# iterating message and exits 1 instead of a silent SIGSEGV. The `.stderr`
# sidecar pins the located IterBase message (#1492 shape), including the source
# line and the file basename; the caught twin lives in `null_deref_value_caught`.
struct Ints:
    list[int64] items = None

def main() -> int64:
    Ints x = Ints()
    print("reached")
    for y in x.items:
        print(y)
    return 0
