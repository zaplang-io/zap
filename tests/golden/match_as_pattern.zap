# Golden fixture: `as` sub-pattern capture in `match` (#603, epic #614 D11).
# `case <pattern> as name:` matches the inner pattern and, on success, binds
# `name` to the *whole* matched value. The inner pattern still destructures, so
# the nested captures bind alongside the `as` name, and the capture is in scope
# in the arm body AND in the arm's guard. Exercised here: over a sequence (binds
# `pair`/`x`/`y`), with a guard that reads the capture (`len(pair) == 2`), over a
# struct pattern (binds `origin`), and over an or-pattern (`1 | 2 | 3 as n`).
struct Point:
    int64 x
    int64 y


def describe(list[int64] xs):
    match xs:
        case [x, y] as pair if len(pair) == 2:
            print("pair", x, y, len(pair))
        case [x, y, z] as triple:
            print("triple", x, z, len(triple))
        case _:
            print("other")


def at_origin(Point p):
    match p:
        case Point(x=0) as origin:
            print("origin", origin.x, origin.y)
        case Point(x=x, y=y) as elsewhere:
            print("elsewhere", elsewhere.x, elsewhere.y)


def classify(int64 v):
    match v:
        case 1 | 2 | 3 as n:
            print("small", n)
        case other as whole:
            print("big", other, whole)


def main():
    describe([10, 20])
    describe([1, 2, 3])
    describe([5])
    at_origin(Point(0, 7))
    at_origin(Point(4, 9))
    classify(2)
    classify(42)
