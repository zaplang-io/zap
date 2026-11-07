# diff: match
# String concatenation, lexicographic comparison (by codepoint, so uppercase
# sorts before lowercase), and substring membership match CPython (epic #374,
# walled garden). Comparison/membership results are mapped to ints via mark() so
# the booleans are printed without relying on bool rendering.
def mark(bool b) -> int64:
    if b:
        return 1
    return 0


def main():
    print("abc" + "def")
    print("" + "x")
    print(mark("a" < "b"))
    print(mark("B" < "a"))
    print(mark("abc" < "abd"))
    print(mark("abc" == "abc"))
    print(mark("abc" != "abd"))
    print(mark("bc" in "abc"))
    print(mark("x" in "abc"))
    print(mark("" in "abc"))
