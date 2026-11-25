# Golden fixture (#1489 Epic, #1493 part B): an *uncaught* method call on a null
# built-in reference value reports the located `NullReferenceError` and exits 1.
# The `.stderr` sidecar pins the located MethodReceiver message, including the
# source line and the file basename.
struct Box:
    str name = None

def main() -> int64:
    Box b = Box()
    print("reached")
    auto u = b.name.upper()
    print(u)
    return 0
