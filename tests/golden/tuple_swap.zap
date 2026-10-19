# Golden fixture: positional tuple destructuring in a declaration, and the swap
# idiom via tuple reassignment `(x, y) = (y, x)` (D-0049). The right-hand tuple
# is fully evaluated before any target is stored, so the swap is correct.
def main():
    auto (a, b, c) = (1, 2, 3)
    print(a)
    print(b)
    print(c)
    auto x = 7
    auto y = 9
    (x, y) = (y, x)
    print(x)
    print(y)
