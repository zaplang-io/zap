# Golden fixture (#1489 Epic, #1493 part A): the Epic's headline footgun —
# `len(n.name)` where the non-`Optional` `str` field was left null — now reports
# a located `NullReferenceError` and exits 1 instead of a silent SIGSEGV. The
# `.stderr` sidecar pins the located BuiltinLen message, including the source
# line and the file basename.
struct Node:
    str name = None

def main() -> int64:
    Node n = Node()
    print("reached")
    auto k = len(n.name)
    print(k)
    return 0
