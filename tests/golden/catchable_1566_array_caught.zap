# Golden fixture (#1566, PR2): numeric array[T] N-D indexing and slicing bounds
# are catchable — an out-of-range index raises IndexError and a zero-step slice
# raises ValueError, both handled by a local `except` with `finally` running and
# execution continuing. One failing operation per `try`. The uncaught form is
# `catchable_1566_array_uncaught.zap`.
def main() -> int64:
    auto a = array[int64]([1, 2, 3])
    # 1-D read out of range -> IndexError.
    int64 i = 9
    try:
        auto v = a[i]
    except IndexError as e:
        print("get")
    finally:
        print("fin")
    # 1-D write out of range -> IndexError.
    try:
        a[i] = 7
    except IndexError as e:
        print("set")
    # Full N-D integer index out of range -> IndexError (per-axis check).
    auto m = array[int64]([[1, 2], [3, 4]])
    int64 j = 2
    try:
        auto w = m[0, j]
    except IndexError as e:
        print("ndget")
    # Partial index (rank-reduced view) out of range -> IndexError.
    int64 r = 5
    try:
        auto row = m[r]
    except IndexError as e:
        print("partial")
    # Mixed slice + out-of-range integer axis -> IndexError.
    try:
        auto col = m[:, j]
    except IndexError as e:
        print("mixed")
    # Zero-step slice -> ValueError.
    int64 st = 0
    try:
        auto s = a[::st]
    except ValueError as e:
        print("slice")
    print("continued")
    return 0
