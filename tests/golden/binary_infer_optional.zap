# Binary expressions infer a generic type argument exactly as bare literals do
# (#1128), mirroring sema's binary result-type rules. The argument `2 - 5`
# parses as a subtraction, so monomorphization must type it `int64` to pin
# `Optional`'s `T`; an untyped `auto count = 2 - 5` binding then carries that
# type into a later constructor (the issue's exact shape). This pins the value
# *and inferred type* round-trips across the operator families: integer
# arithmetic stays `int64`, true division is `float64` (not the integer operand
# type, D-0024), mixed int/float promotes to `float64`, bitwise/shift stay
# `int64`, comparison/logical/membership are `bool`, and `+` on strings
# concatenates to `str`. Values are chosen to format exactly.
struct Box[T]:
    T item

def wrap[T](T x) -> Optional[T]:
    return Optional.Some(x)

def main():
    # Integer arithmetic -> int64
    print(Optional.Some(2 - 5).unwrap())  # -3
    print(Optional.Some(2 + 3).unwrap())  # 5
    print(Optional.Some(4 * 3).unwrap())  # 12
    print(Optional.Some(7 // 2).unwrap())  # 3
    print(Optional.Some(7 % 3).unwrap())  # 1
    print(Optional.Some(2 ** 3).unwrap())  # 8
    # True division -> float64 (never the integer operand type, D-0024)
    print(Optional.Some(6 / 2).unwrap())  # 3.0
    # Float and mixed int/float arithmetic -> float64
    print(Optional.Some(2.5 + 1.0).unwrap())  # 3.5
    print(Optional.Some(3 + 0.5).unwrap())  # 3.5
    # Bitwise / shift -> int64
    print(Optional.Some(6 & 3).unwrap())  # 2
    print(Optional.Some(1 << 4).unwrap())  # 16
    # Comparison / logical / membership -> bool
    print(Optional.Some(2 < 5).unwrap())  # True
    print(Optional.Some(True and False).unwrap())  # False
    print(Optional.Some(3 in [1, 2, 3]).unwrap())  # True
    # `+` concatenation -> str
    print(Optional.Some("a" + "b").unwrap())  # ab
    # The auto-binding propagation path (the issue's exact shape)
    auto count = 2 - 5
    print(Optional.Some(count).unwrap())  # -3
    # General: the same inferer drives a user generic struct and a generic fn
    print(Box(2 - 5).item)  # -3
    print(wrap(10 - 1).unwrap())  # 9
