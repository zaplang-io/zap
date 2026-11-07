# Golden fixture: interface-value rendering through the Display protocol
# (#707, D-0124). An interface value is a `{data, itable}` fat pointer; rendering
# it dynamically dispatches through the itable's renderer slot to the CONCRETE
# type's renderer (the struct's compiler-default field-wise form here), exactly
# as `print`/`str`/f-string render a concrete value. Before #707 rendering an
# interface value was a clean ZAP6001 deferral. Two implementors behind one
# interface variable each dispatch to their OWN renderer — proving the render
# dispatch is dynamic, like interface method dispatch (D-0016).

interface Shape:
    def area(self) -> f64

struct Circle:
    f64 r

    def area(self) -> f64:
        return 3.14159 * self.r * self.r

struct Square:
    f64 side

    def area(self) -> f64:
        return self.side * self.side

# An interface-typed parameter renders by the same dynamic dispatch.
def show(Shape sh):
    print(sh)

def main():
    Shape s = Circle(r=2.0)
    print(s)  # Circle(r=2.0)
    print(str(s))  # Circle(r=2.0)   str() matches print
    print(f"[{s}]")  # [Circle(r=2.0)]  f-string matches print
    # Re-bind the SAME interface variable to a different concrete type: the
    # render follows the concrete value behind the fat pointer, not the static
    # interface type.
    s = Square(side=3.0)
    print(s)  # Square(side=3.0)
    # Dispatch through an interface-typed parameter, two implementors.
    show(Circle(r=1.0))  # Circle(r=1.0)
    show(Square(side=4.0))  # Square(side=4.0)
