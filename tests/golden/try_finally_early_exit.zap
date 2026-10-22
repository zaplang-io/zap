# Golden (I#185, guardrail for D-0069): bare `try ... finally` must run `finally`
# on EARLY exits — `return`, `break`, and `continue` — not only on normal/unwind
# paths. I#22 deferred this; I#185's cleanup stack closes the gap so `finally` is
# Python-faithful (always runs). Three independent checks below.
def on_return() -> int64:
    try:
        print("r-body")
        return 1
    finally:
        print("r-finally")
    return 0

def on_break():
    for i in range(0, 3):
        try:
            print("b-body")
            break
        finally:
            print("b-finally")

def on_continue():
    for i in range(0, 2):
        try:
            print("c-body")
            continue
        finally:
            print("c-finally")

def main() -> int64:
    print(on_return())
    on_break()
    on_continue()
    return 0
