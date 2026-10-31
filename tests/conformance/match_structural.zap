# claim: MATCH-1
# SPEC §6 (I#474): nested structural patterns, rest captures, and guards.
struct Point:
    int64 x
    int64 y

enum Inner:
    Value(int64 n)
    Empty

enum Outer:
    Child(Inner item)
    Position(Point p)
    Empty

def describe(Outer item):
    match item:
        case Outer.Child(Inner.Value(0)):
            print("zero")
        case Outer.Child(Inner.Value(n)) if n > 10:
            print("large", n)
        case Outer.Child(Inner.Value(n)):
            print("value", n)
        case Outer.Child(Inner.Empty):
            print("inner empty")
        case Outer.Position(Point(x=0, y=y)):
            print("axis", y)
        case Outer.Position(_):
            print("point")
        case _:
            print("outer empty")

def main():
    auto pair = (1, (2, 3))
    match pair:
        case (a, b, c):
            print("wrong tuple arity", a, b, c)
        case (0, _):
            print("wrong tuple")
        case (1, (a, b)):
            print("nested tuple", a, b)
    match (2, 9):
        case (1, n) | (2, n):
            print("or binding", n)
    auto triple = (1, "middle", 3)
    match triple:
        case (head, *middle, tail):
            print("tuple rest", head, middle[0], tail)
    list[int64] values = [1, 2, 3, 4]
    int64 x = 99
    match values:
        case [1, 2]:
            print("wrong length")
        case [x, *rest] if x > 10:
            print("wrong guard")
        case [first, *rest, last]:
            print("list rest", first, len(rest), rest[0], last)
    print("outer binding", x)
    match values:
        case [first, *rest]:
            print("trailing rest", first, len(rest), rest[2])
    list[int64] empty = []
    match empty:
        case [first, *rest]:
            print("wrong empty")
        case []:
            print("empty list")
    match [7]:
        case [7, *rest]:
            print("empty rest", len(rest))
    describe(Outer.Child(Inner.Value(0)))
    describe(Outer.Child(Inner.Value(20)))
    describe(Outer.Child(Inner.Value(3)))
    describe(Outer.Child(Inner.Empty))
    describe(Outer.Position(Point(0, 8)))
    describe(Outer.Position(Point(2, 8)))
    describe(Outer.Empty)
    Optional[Point] present = Optional.Some(Point(0, 5))
    match present:
        case Optional.Some(Point(x=0, y=y)):
            print("niche point", y)
        case _:
            print("wrong niche")
    Optional[Point] absent = None
    match absent:
        case Optional.Some(Point(x=0, y=y)):
            print("wrong null payload", y)
        case _:
            print("niche empty")
