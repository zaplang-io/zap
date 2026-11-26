# diff: match
# #1648/#1656: ljust/rjust/center repeat the whole fill codepoint (valid UTF-8)
# and center splits odd padding the CPython way (extra fill on the left when both
# width and margin are odd). Every case here agrees with CPython, including the
# odd-width centered cases that were previously excluded (#1656).
def main():
    print("x".ljust(3, "é"))
    print("x".rjust(3, "é"))
    print("x".center(3, "é"))
    print("x".ljust(3, "€"))
    print("x".rjust(3, "😀"))
    print("αβ".ljust(4, "γ"))
    print("αβ".rjust(4, "γ"))
    print("é".center(5, "α"))
    print("x".center(4, "é"))
    print("hi".rjust(5, "*"))
    print(len("x".ljust(3, "😀")))
    print("".ljust(3, "é"))
    # odd-width centered padding (now matches CPython's left-biased split, #1656)
    print(f"[{'hi'.center(5)}]")
    print(f"[{'ab'.center(5, '-')}]")
    print("x".center(5, "é"))
    print("abc".center(6, "·"))
