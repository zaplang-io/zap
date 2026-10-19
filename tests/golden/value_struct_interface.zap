# Golden fixture: a `value struct` behind an interface value (I#75 Part 1, D-0051
# + D-0016). An interface value is a `{data, itable}` fat pointer whose `data`
# word must hold a pointer, but a value struct has no stable address, so on the
# concrete -> interface conversion it is BOXED: copied to a fresh heap instance
# that `data` points at. The box is an independent copy (value semantics) and a
# normal GC object (traced under both collectors).
#
# This fixture proves two things:
#   1. Dynamic dispatch works for a boxed value struct, and is dynamic across two
#      value-struct implementors (Rect, Square) at the same call site.
#   2. The boxed copy is independent of the original: mutating the source after
#      the conversion does not change what the interface value sees.

interface Shape:
    def area(self) -> f64
    def name(self) -> str

value struct Rect:
    f64 w
    f64 h

    def area(self) -> f64:
        return self.w * self.h

    def name(self) -> str:
        return "rect"

value struct Square:
    f64 side

    def area(self) -> f64:
        return self.side * self.side

    def name(self) -> str:
        return "square"

# Takes its argument as an interface value (by-value fat pointer over a boxed
# value struct) and dispatches through it — the same body serves every
# implementor.
def describe(Shape s) -> f64:
    print(s.name())
    return s.area()

def main():
    # Boxing independence: `s` holds a heap copy of `r` taken at the conversion.
    auto r = Rect(w=2.0, h=3.0)
    Shape s = r
    r.w = 100.0          # mutate the ORIGINAL after it was boxed behind `s`
    r.h = 100.0
    print(s.area())      # 6.000000      — the boxed copy is unaffected
    print(r.area())      # 10000.000000  — the original did change
    # Dynamic dispatch through an interface-typed parameter, two implementors.
    print(describe(Rect(w=4.0, h=5.0)))   # rect / 20.000000
    print(describe(Square(side=6.0)))     # square / 36.000000
