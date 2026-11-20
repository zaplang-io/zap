# Monomorphization pins a generic argument from sema's inferred type when its
# own best-effort inferer cannot (#1167, epic #1169): a module constant
# (`math.inf`, #1162) and built-in call returns (`len`, `str.upper`, `list.pop`,
# #1132) all previously failed with ZAP6001. This pins the value round-trips
# through unwrap(), proving the inferred instance is the right one end-to-end.
import math

def main():
    list[int64] xs = [1, 2, 3]
    print(Optional.Some(len(xs)).unwrap())  # 3  (built-in free call -> int64)
    print(Optional.Some("hi".upper()).unwrap())  # HI  (built-in method -> str)
    print(Optional.Some(xs.pop()).unwrap())  # 3  (built-in method -> int64)
    print(Optional.Some(math.inf).is_some())  # True  (module constant -> float64)
