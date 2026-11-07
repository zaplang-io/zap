# Golden fixture: a single (bare) failure matches `except* E`, treated as a
# one-element group (#605, D-0129). The `as` binding is ALWAYS a group, so
# `.exceptions` has exactly one entry even though only one exception was raised.
def main() -> int64:
    print("start")
    try:
        raise ValueError("solo")
    except* ValueError as eg:
        print("caught:")
        for m in eg.exceptions:
            print(m)
    print("done")
    return 0
