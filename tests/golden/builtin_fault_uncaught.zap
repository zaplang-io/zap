# Golden fixture: an *uncaught* built-in fault (index-out-of-range) still
# terminates the process non-zero with a message (#383). The fault unwinds
# through `zap_panic` to the top-level handler (`zap_uncaught`), which reports
# it and exits 1 — the diagnostic must not be silently swallowed now that these
# faults are catchable. Buffered stdout ("reached") is flushed on exit.
def main() -> int64:
    list[int64] xs = [1, 2]
    print("reached")
    auto v = xs[7]
    print(v)
    return 0
