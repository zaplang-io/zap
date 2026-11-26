# Golden fixture (#1579): an *uncaught* empty `heap.replace(x)` still reports the
# normal Zap exception diagnostic and exits 1 — it is not silently swallowed now
# that it is catchable. With no handler, the `IndexError` unwinds to the
# top-level handler (`zap_uncaught`), which reports "heap replace on an empty
# heap" and exits 1. Buffered stdout is flushed on exit.
def main() -> int64:
    print("reached")
    auto h = heap[int64]()
    auto v = h.replace(1)
    print(v)
    return 0
