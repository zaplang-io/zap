# Golden fixture: `except*` exception-group handling (#605, D-0129). A `scope`
# with two concurrent failures surfaces an aggregate `ExceptionGroup` (D-0045);
# two `except*` clauses each peel the sub-exceptions matching their type into a
# sub-group, bind it, and iterate it via `.exceptions`. BOTH clauses run (unlike
# plain `except`, which runs only the first match). Children are rendered in a
# stable sorted order, so output is deterministic regardless of scheduling.
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
    except* TimeoutError as timeouts:
        print("timeouts:")
        for m in timeouts.exceptions:
            print(m)
    except* ValueError as values:
        print("values:")
        for m in values.exceptions:
            print(m)
    print("done")
    return 0
