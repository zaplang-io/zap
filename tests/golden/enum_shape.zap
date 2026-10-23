# Golden fixture: tier-3 data-carrying variants / sum types (D-0046). `Shape` is
# a tagged value `{ tag | payload }` sized to the largest variant and copied by
# value. Construction is qualified with named OR positional args; `match`
# destructures positionally OR by name, and is exhaustive without a `_`.
enum Shape:
    Circle(f64 r)
    Rect(f64 w, f64 h)
    Unit

def area(Shape s) -> f64:
    match s:
        case Shape.Circle(r):  # positional binding
            return 3.14159 * r * r
        case Shape.Rect(w=w, h=h):  # named binding
            return w * h
        case Shape.Unit:  # payload-free variant
            return 1.0

def main():
    print(area(Shape.Circle(r=2.0)))  # 12.566360
    print(area(Shape.Rect(3.0, 4.0)))  # 12.000000
    print(area(Shape.Unit))  # 1.000000
