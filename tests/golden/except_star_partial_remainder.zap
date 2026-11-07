# Golden fixture: a partial `except*` match re-raises the single unmatched
# remainder BARE (Zap's bare-for-single rule, #605/D-0129), so a plain enclosing
# `except E` catches it. Here `except* ValueError` peels the ValueError and the
# lone TimeoutError remainder propagates to the outer `except TimeoutError`.
def fail_timeout():
    raise TimeoutError("t-out")

def fail_value():
    raise ValueError("bad-value")

def main() -> int64:
    print("start")
    try:
        try:
            scope:
                spawn fail_timeout()
                spawn fail_value()
        except* ValueError as values:
            print("handled values:")
            for m in values.exceptions:
                print(m)
    except TimeoutError as e:
        print("outer caught timeout:")
        print(e)
    print("done")
    return 0
