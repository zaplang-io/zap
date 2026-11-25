# Golden fixture (#1489 Epic, #1493): an *uncaught* field read on a null
# reference unwinds to the top-level handler, which reports the located
# `NullReferenceError` message and exits 1 — the diagnostic is not silently
# swallowed. Buffered stdout ("reached") is flushed on exit. The `.stderr`
# sidecar pins the located message, including the source line and the file
# basename (#1492's located shape: `... at <file>:<line>`).
struct Point:
    int64 x
    int64 y

def main() -> int64:
    Point p = None
    print("reached")
    auto v = p.x
    print(v)
    return 0
