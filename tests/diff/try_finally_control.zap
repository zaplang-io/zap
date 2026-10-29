# diff: match
# `finally` must run on early exits — return, break, continue — not only on the
# normal/unwind path (I#185, D-0069). CPython-faithful ordering.
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


def main():
    print(on_return())
    on_break()
    on_continue()
