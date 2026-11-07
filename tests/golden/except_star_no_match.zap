# Golden fixture: when no `except*` clause matches, the whole `ExceptionGroup`
# propagates unchanged (#605, D-0129). Here it is uncaught, so the program exits
# non-zero and renders the aggregate group to stderr (sorted sub-exceptions).
def fail_timeout():
    raise TimeoutError("t-out")

def fail_value():
    raise ValueError("bad-value")

def main() -> int64:
    print("start")
    try:
        scope:
            spawn fail_timeout()
            spawn fail_value()
    except* KeyError:
        print("unreachable")
    return 0
