# Golden fixture (#1675): a generic matches a value of an enum type parameter.
# The variant arms (Circle, Square) are exhaustive, so the trailing `case _:` —
# whose body treats the value as a callable, type-correct only for a non-enum
# specialization — is statically unreachable and must be elided before lowering
# (before the fix it failed with ZAP4001, "value of type enum#0 is not a
# function"). The enum analog of #1668's single-irrefutable-arm elision.

enum Shape:
    Circle
    Square

def describe[T](T value) -> str:
    match value:
        case Shape.Circle:
            return "circle"
        case Shape.Square:
            return "square"
        case _:
            fn() -> str f = value
            return f()

def main():
    print(describe[Shape](Shape.Circle))
    print(describe[Shape](Shape.Square))
