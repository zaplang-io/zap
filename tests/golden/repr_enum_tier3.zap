# Golden fixture: a tier-3 data-carrying enum (D-0046) renders structurally
# under the general Display protocol (#274, D-0124; tier-3 added in #706) —
# `EnumName.Variant(field=value, …)` for a data-carrying variant and
# `EnumName.Variant` for a payload-free one — consistently across print(),
# str(), and f-strings. Before #706 this was a clean ZAP6001 deferral error.
enum Shape:
    Circle(f64 r)
    Rect(f64 w, f64 h)
    Unit


def main():
    # Each variant, via print(): named and positional construction render the
    # same structural form (fields are always labelled `name=value`).
    print(Shape.Circle(r=2.0))  # Shape.Circle(r=2.0)
    print(Shape.Rect(3.0, 4.0))  # Shape.Rect(w=3.0, h=4.0)
    print(Shape.Unit)  # Shape.Unit  (payload-free: no parentheses)
    # str() and f-strings use the same renderer.
    print(str(Shape.Circle(r=2.0)))  # Shape.Circle(r=2.0)
    print(f"s={Shape.Rect(w=1.0, h=2.0)}")  # s=Shape.Rect(w=1.0, h=2.0)
    print(f"[{Shape.Unit}]")  # [Shape.Unit]
