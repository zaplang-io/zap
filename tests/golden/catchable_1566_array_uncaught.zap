# Golden fixture (#1566, PR2): an *uncaught* array index-out-of-range still
# reports the normal Zap diagnostic and exits 1 (not silently swallowed now that
# it is catchable). `a[9]` on a length-3 array unwinds to the top-level handler.
def main() -> int64:
    auto a = array[int64]([1, 2, 3])
    print("reached")
    int64 i = 9
    auto v = a[i]
    print(v)
    return 0
