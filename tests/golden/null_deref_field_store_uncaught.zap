# Golden fixture (#1489 Epic, #1493): an *uncaught* field store on a null
# reference reports the located `NullReferenceError` write message and exits 1.
# The `.stderr` sidecar pins the located write-shape message, including the
# source line and the file basename.
struct Box:
    int64 v

def main() -> int64:
    Box b = None
    print("reached")
    b.v = 5
    print("unreached")
    return 0
