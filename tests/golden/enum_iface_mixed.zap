# Golden fixture: a struct, a tier-1 enum, and a tier-3 enum ALL conform to the
# same interface and dispatch polymorphically through one call site (#731,
# D-0016). This proves the three concrete representations — a reference struct
# pointer, a boxed tier-1 scalar, and a boxed tier-3 tagged-struct pointer — all
# sit behind the same `{data, itable}` fat pointer and dispatch to their own
# method, uniformly ("any type with the method set satisfies the interface").
#
# NOTE: a heterogeneous `list[Shape]` literal/append is NOT used here — storing a
# concrete value into an interface-typed collection element is a separate,
# pre-existing interface limitation (I#84 follow-up) that affects structs too, so
# it is out of scope for #731. Polymorphism is shown via a rebindable
# interface-typed binding and an interface-typed parameter instead.

interface Shape:
    def sides(self) -> int

struct Tri:
    def sides(self) -> int:
        return 3

enum Poly:  # tier-1 (unboxed integer)
    Quad
    Pent

    def sides(self) -> int:
        match self:
            case Poly.Quad:
                return 4
            case _:
                return 5

enum Spur:  # tier-3 (data-carrying)
    Star(int points)

    def sides(self) -> int:
        match self:
            case Spur.Star(points):
                return points

def sides_of(Shape s) -> int:
    return s.sides()

def main():
    # One interface-typed binding, rebound across all three representations.
    Shape s = Tri()
    print(s.sides())  # 3   (struct)
    s = Poly.Quad
    print(s.sides())  # 4   (tier-1 enum, boxed scalar)
    s = Poly.Pent
    print(s.sides())  # 5
    s = Spur.Star(points=7)
    print(s.sides())  # 7   (tier-3 enum, boxed tagged struct)

    # One parameterized call site, every representation.
    print(sides_of(Tri()))  # 3
    print(sides_of(Poly.Quad))  # 4
    print(sides_of(Spur.Star(points=9)))  # 9
