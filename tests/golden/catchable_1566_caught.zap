# Golden fixture (#1566): recoverable input/state faults raised inside `try`
# are caught by a matching `except` with the right exception type, the `finally`
# runs, and execution continues past the handler — they no longer abort or
# `exit(1)` the process. One failing operation per `try` (the inventory's rule),
# covering the scalar / string / bytes / sequence / stdlib surfaces of the
# first-cut fix. The uncaught counterpart is `catchable_1566_uncaught.zap`.
def main() -> int64:
    # str.index: absent substring -> ValueError (SPEC §9.2).
    try:
        auto i = "x".index("y")
        print(i)
    except ValueError as e:
        print("str.index")
    finally:
        print("fin")
    # bytes indexing out of range -> IndexError.
    bytes b = b"x"
    int64 bi = 5
    try:
        auto v = b[bi]
    except IndexError as e:
        print("bytes.index")
    # chr() of an invalid Unicode scalar -> ValueError.
    int64 n = -1
    try:
        auto c = chr(n)
    except ValueError as e:
        print("chr")
    # list slice with a zero step -> ValueError.
    list[int64] xs = [1, 2, 3]
    int64 st = 0
    try:
        auto sl = xs[::st]
    except ValueError as e:
        print("list.slice")
    # pop from an empty deque -> IndexError.
    auto d = deque[int64]()
    try:
        auto dv = d.pop()
    except IndexError as e:
        print("deque.pop")
    # pop from an empty heap -> IndexError.
    auto h = heap[int64]()
    try:
        auto hv = h.pop()
    except IndexError as e:
        print("heap.pop")
    # random.randint with reversed bounds -> ValueError.
    try:
        auto r = random.randint(2, 1)
    except ValueError as e:
        print("randint")
    print("continued")
    return 0
