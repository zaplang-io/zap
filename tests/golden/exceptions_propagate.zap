# Golden fixture: an exception raised in a callee unwinds *through* that callee's
# frame to a `try` in the caller (#22). `boom` has no handler, so its frame is
# unwound (DWARF CFI / unwind tables) and control lands at main's landing pad.
def boom(int64 n) -> int64:
    if n > 0:
        raise "deep failure"
    return n

def main() -> int64:
    try:
        auto x = boom(5)
        print(x)
    except Exception as e:
        print("caught in main:")
        print(e)
    return 0
