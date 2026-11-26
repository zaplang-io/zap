# diff: match
# #1648: ljust/rjust/center repeat the whole fill codepoint, producing valid
# UTF-8 that matches CPython. Only cases where Zap and CPython agree appear here:
# ljust/rjust always agree; center is included only where its padding split
# matches CPython (even width, or even padding) — Zap's odd-width centered split
# puts the smaller half on the left, a separate pre-existing divergence.
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
