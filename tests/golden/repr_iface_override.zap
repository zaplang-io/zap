# Golden fixture: an explicit `__str__`/`__repr__` on the concrete type STILL
# overrides when the value is rendered through an interface (#707, D-0124). The
# renderer slot resolves the same way a direct render does — `__str__`
# (preferred), else `__repr__`, else the compiler default — so a hand-written
# renderer wins through the type-erased interface value, and two implementors
# each dispatch to their own override.

interface Shape:
    def area(self) -> f64

struct Circle:
    f64 r

    def area(self) -> f64:
        return self.r

    def __str__(self) -> str:
        return f"<circle r={self.r}>"

struct Square:
    f64 side

    def area(self) -> f64:
        return self.side

    # Only `__repr__`: str()/print fall back to it when there is no `__str__`.
    def __repr__(self) -> str:
        return "SQ"

def main():
    Shape a = Circle(r=2.0)
    print(a)  # <circle r=2.0>   __str__ override wins through the interface
    print(str(a))  # <circle r=2.0>
    Shape b = Square(side=3.0)
    print(b)  # SQ               __repr__ override wins when no __str__
    print(f"{b}")  # SQ
