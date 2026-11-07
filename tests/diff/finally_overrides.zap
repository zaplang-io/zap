# diff: match
# A `return` inside `finally` overrides a `return` in the `try` body, matching
# CPython (epic #374, walled garden): `f()` returns 2, not 1. This is a classic
# point of compiler divergence, so it is pinned explicitly.
def f() -> int64:
    try:
        return 1
    finally:
        return 2


def main():
    print(f())
