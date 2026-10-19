# Golden fixture: interface values & dynamic dispatch (interfaces, #84, D-0016).
# An interface value is a `{data, itable}` fat pointer (design/0006 §2.4): a
# concrete struct used where an interface is expected is wrapped with the itable
# for that (struct, interface) pair, and an interface-method call dispatches
# through the itable at runtime. Two implementors (Circle, Square) prove the
# dispatch is dynamic — the same `s.area()` / `s.name()` call site runs a
# different method depending on the concrete value behind the interface.

interface Shape:
    def area(self) -> f64
    def name(self) -> str

struct Circle:
    f64 r

    def area(self) -> f64:
        return 3.141592653589793 * self.r * self.r

    def name(self) -> str:
        return "circle"

struct Square:
    f64 side

    def area(self) -> f64:
        return self.side * self.side

    def name(self) -> str:
        return "square"

# Takes its argument as an interface value (by-value fat pointer) and dispatches
# through it — the same body serves every implementor.
def describe(Shape s) -> f64:
    print(s.name())
    return s.area()

def main():
    # Dispatch through interface-typed bindings.
    Shape a = Circle(r=2.0)
    Shape b = Square(side=3.0)
    print(a.name())     # circle
    print(a.area())     # 12.566371  (pi * 2 * 2)
    print(b.name())     # square
    print(b.area())     # 9.000000   (3 * 3)
    # Dispatch through an interface-typed parameter.
    print(describe(a))  # circle / 12.566371
    print(describe(b))  # square / 9.000000
