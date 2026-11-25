# Golden fixture (#1489 Epic, #1541): an *uncaught* method call on a null user
# reference-struct receiver reports the located `NullReferenceError` and exits 1.
# The `.stderr` sidecar pins the located MethodReceiver message, with the source
# line and the file basename.
struct Thing:
    int64 x

    def poke(self):
        self.x = self.x + 1

def main() -> int64:
    Thing t = None
    print("reached")
    t.poke()
    print("unreached")
    return 0
