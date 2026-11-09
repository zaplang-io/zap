# Golden fixture: `Optional[T]` is hashable when `T` is (#704). With `float64` a
# first-class dict key (#800/#803) and the capability model (D-0139/#808) routing
# a float element through the shared #803 key word, `Optional[float]` is now a
# valid dict key / set element — `None` and a present float are both hashable.
# (This supersedes the former `optional_float_key_reject` compile-fail fixture:
# the capability model lifts that rejection; a genuinely unhashable payload such
# as `Optional[list[int64]]` is still rejected.)
def main():
    dict[Optional[float], int64] d = {}
    Optional[float] k = Optional.Some(1.5)
    d[k] = 1
    print(len(d))  # 1
