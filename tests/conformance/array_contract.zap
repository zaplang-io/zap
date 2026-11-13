# claim: ARR-1
# SPEC §15 (I#476): supported one-dimensional array operations and views.
def main():
    auto empty = array[f64]([])
    print("empty", len(empty), sum(empty))
    auto a = array[i64]([1, 2, 3, 4, 5])
    auto b = array[int64]([5, 4, 3, 2, 1])
    print("ops", sum(a + b), sum(a - b), sum(a * b))
    print("broadcast", sum(a + 2), sum(a - 1), sum(2 * a))
    auto f = array[float64]([0.5, 1.5, 2.5, 3.5, 4.5])
    print("promotion", sum(a + f), sum(a + 0.5))
    auto bytes = array[u8]([250, 10])
    print("bytes", sum(bytes), sum(bytes + 10), bytes[0])
    auto flags = array[bool]([True, False, True])
    print("bool", sum(flags), sum(flags + flags))
    auto view = a[1:4:1]
    view[0] = 20
    a[3] = 40
    print("view", len(view), view[0], view[2], a[1])
    auto nested = view[1:]
    nested[0] = 30
    print("nested", a[2], sum(nested + 1))
    auto none = a[4:2]
    print("empty view", len(none), sum(none))
    try:
        # (5,) + (3,) is not broadcast-compatible (neither is 1), so it raises
        # even under #501 broadcasting; a length-1 operand would instead stretch.
        auto mismatch = a + array[i64]([1, 2, 3])
        print("wrong length", sum(mismatch))
    except ValueError as e:
        print("shape error", e)
    try:
        # (0,) + (2,) is incompatible; a length-1 operand would stretch to empty.
        auto mismatch = empty * array[f64]([1.0, 2.0])
        print(sum(mismatch))
    except ValueError:
        print("empty shape error")
