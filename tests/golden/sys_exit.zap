# Golden fixture: sys.exit terminates with the given code; code after it does
# not run (ticket #97).
def main() -> int64:
    print("before")
    sys.exit(3)
    print("after")
    return 0
