# Subscript, slice, tuple-literal, and comprehension arguments infer a generic
# type argument from information already present (#1131), siblings of #1117
# (unary) and #1128 (binary). A subscript yields the base container's
# element/value type, a slice yields the base's own type, a tuple literal yields
# tuple[...], and a list/set/dict comprehension yields list/set/dict of its
# element. This pins the value and inferred-type round-trips through unwrap().
# dict/set iteration order is unspecified (#1066), so set/dict results are
# probed by is_some()/key-lookup rather than printed as a whole.
struct Box[T]:
    T item

def wrap[T](T x) -> Optional[T]:
    return Optional.Some(x)

def main():
    # Subscript -> element / value type
    list[int64] xs = [10, 20, 30]
    print(Optional.Some(xs[0]).unwrap())  # 10
    str s = "hi"
    print(Optional.Some(s[0]).unwrap())  # h  (str index -> char)
    dict[str, int64] d = {"k": 7}
    print(Optional.Some(d["k"]).unwrap())  # 7  (dict value type)
    auto data = b"A"
    print(Optional.Some(data[0]).unwrap())  # 65  (bytes index -> int64)
    # Slice -> the base container's own type
    print(Optional.Some(xs[0:2]).unwrap())  # [10, 20]
    print(Optional.Some(s[0:2]).unwrap())  # hi
    # Tuple literal -> tuple[...]
    print(Optional.Some((1, 2)).unwrap())  # (1, 2)
    print(Optional.Some((1, "a")).unwrap())  # (1, 'a')
    # List comprehension -> list[elem]
    print(Optional.Some([x * 2 for x in xs]).unwrap())  # [20, 40, 60]
    # Set / dict comprehensions infer; order is unspecified, so probe
    print(Optional.Some({x for x in xs}).is_some())  # True
    auto dc = Optional.Some({x: x * x for x in xs})
    print(dc.unwrap()[10])  # 100  (dict[int64, int64] built from a list)
    # The auto-binding propagation path (the issue's exact shape)
    auto e = xs[0]
    print(Optional.Some(e).unwrap())  # 10
    # General: the same inferer drives a user generic struct and a generic fn
    print(Box(xs[1]).item)  # 20
    print(wrap([x for x in xs]).unwrap())  # [10, 20, 30]
