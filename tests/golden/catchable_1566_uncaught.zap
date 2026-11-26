# Golden fixture (#1566): an *uncaught* recoverable fault still reports the
# normal Zap exception diagnostic and exits 1 — it is not silently swallowed now
# that these faults are catchable. `"x".index("y")` raises `ValueError` and,
# with no handler, unwinds to the top-level handler (`zap_uncaught`), which
# reports "substring not found" and exits 1. Buffered stdout is flushed on exit.
def main() -> int64:
    print("reached")
    auto i = "x".index("y")
    print(i)
    return 0
