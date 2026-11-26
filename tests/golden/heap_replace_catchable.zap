# Golden fixture (#1579): empty `heap.replace(x)` raises a catchable
# `IndexError` (superseding the D-0236 abort), matching `heap.pop` / `heap.peek`.
# A matching `except IndexError` handles it, the `finally` runs, and execution
# continues past the handler — the process no longer aborts. The uncaught
# counterpart is `heap_replace_uncaught.zap`.
def main() -> int64:
    # replace on an empty i64 heap -> IndexError, caught; finally runs.
    auto h = heap[int64]()
    try:
        auto v = h.replace(1)
        print(v)
    except IndexError as e:
        print("caught empty replace")
    finally:
        print("fin")
    # The heap is still usable after the caught fault.
    h.push(7)
    print(h.replace(9))
    print(h.peek())

    # Same contract on a pointer-word (str) heap.
    auto hs = heap[str]()
    try:
        auto sv = hs.replace("x")
    except IndexError as e:
        print("caught str replace")

    # Same contract on a float heap.
    auto hf = heap[float64]()
    try:
        auto fv = hf.replace(1.5)
    except IndexError as e:
        print("caught f64 replace")

    # `pushpop` is defined on an empty heap and does NOT raise — it returns x
    # unchanged, confirming it is unaffected by the replace contract change.
    auto hp = heap[int64]()
    print(hp.pushpop(42))
    print(len(hp))

    print("continued")
    return 0
