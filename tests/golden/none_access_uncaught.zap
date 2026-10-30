# Golden fixture: an *uncaught* member access on `None` still terminates the
# process non-zero with a message (EXC-4). The fault unwinds through `zap_panic`
# to the top-level handler (`zap_uncaught`), which reports it and exits 1 — the
# diagnostic must not be silently swallowed now that the fault is catchable.
# Buffered stdout ("reached") is flushed on exit.
struct Point:
    int64 x
    int64 y

def main() -> int64:
    Point p = None
    print("reached")
    auto v = p.x
    print(v)
    return 0
