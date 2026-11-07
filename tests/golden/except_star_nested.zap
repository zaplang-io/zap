# Golden fixture: nested `try`/`except*` (#605, D-0129). The inner `try` has
# only an `except* TimeoutError` clause, which does not match the raised
# ValueError, so the lone remainder re-raises BARE and the outer `except*
# ValueError` catches it as a one-element group.
def main() -> int64:
    print("start")
    try:
        try:
            raise ValueError("inner-val")
        except* TimeoutError as t:
            print("inner timeout")
    except* ValueError as v:
        print("outer value:")
        for m in v.exceptions:
            print(m)
    print("done")
    return 0
