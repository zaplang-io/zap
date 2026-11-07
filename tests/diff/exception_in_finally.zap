# diff: match
# An exception raised in a `finally` block replaces the in-flight exception and
# propagates, matching CPython (epic #374, walled garden): the inner `try` raises
# ValueError, its `finally` runs and raises TypeError, and the outer handler
# catches the TypeError. Control markers (not the exception text, which renders
# differently) pin the order: inner, fin, caught.
def main():
    try:
        try:
            print("inner")
            raise ValueError("a")
        finally:
            print("fin")
            raise TypeError("b")
    except Exception:
        print("caught")
