# Golden: a `heap[T]` whose element is a tier-1 (constant) enum orders by the
# backing integer value and runs without aborting (I#818). A tier-1 enum is an
# `i64` discriminant, so the heap must select the integer-word comparator — not
# the pointer/`str` one, which previously crashed the first `push` in
# `zap_str_cmp`. Covers a min-heap (smallest backing value first), a max-heap
# (largest first, via cmp="max"), push/pop/peek, and the capability-model promise
# that `heap[Color]` works out of the box (D-0018 / D-0057 / D-0139).
enum Color:
    Red
    Green
    Blue


def main():
    # Min-heap: pops in ascending backing-value order Red(0) < Green(1) < Blue(2),
    # regardless of push order.
    heap[Color] h = heap[Color]()
    h.push(Color.Blue)
    h.push(Color.Red)
    h.push(Color.Green)
    print(h.pop() == Color.Red)  # True
    print(h.pop() == Color.Green)  # True
    print(h.pop() == Color.Blue)  # True

    # Max-heap: largest backing value first.
    heap[Color] hx = heap[Color](cmp="max")
    hx.push(Color.Red)
    hx.push(Color.Blue)
    hx.push(Color.Green)
    print(hx.pop() == Color.Blue)  # True  (Blue=2 is largest)
    print(hx.peek() == Color.Green)  # True  (Green=1 now largest remaining)
