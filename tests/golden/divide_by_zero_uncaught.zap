# Golden fixture: an *uncaught* integer divide-by-zero still terminates the
# process non-zero with a message (EXC-3). The fault unwinds through `zap_panic`
# to the top-level handler (`zap_uncaught`), which reports it and exits 1 — the
# diagnostic must not be silently swallowed now that the fault is catchable.
# Buffered stdout ("reached") is flushed on exit.
def main() -> int64:
    int64 z = 0
    print("reached")
    auto q = 1 // z
    print(q)
    return 0
