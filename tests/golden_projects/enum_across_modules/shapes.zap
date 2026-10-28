# A data-carrying enum (D-0046) and a function over it, both exported.
enum Shape:
    Circle(f64 r)
    Rect(f64 w, f64 h)
    Unit

def area(Shape s) -> f64:
    match s:
        case Shape.Circle(r):
            return 3.14159 * r * r
        case Shape.Rect(w=w, h=h):
            return w * h
        case Shape.Unit:
            return 1.0
