# I#170: the built-in collections (list / dict / set) interoperate with
# user-defined generics through the SAME monomorphization machinery. To the mono
# pass a collection is an ordinary `Type::Named` node, so `list[T]`/`dict[K,V]`/
# `set[T]` substitute, unify, and (I#170) infer from literal arguments uniformly
# with user generics. The collections themselves stay special-cased (a type-erased
# runtime representation); the re-skin into monomorphized structs was declined as
# it would force a performance regression — see DECISIONS.md D-0064 #3.
struct Pair[A, B]:
    A first
    B second

struct Box[T]:
    T value

def firstOf[T](list[T] xs) -> Optional[T]:
    if len(xs) > 0:
        return Optional.Some(xs[0])
    return None

def countDict[K, V](dict[K, V] d) -> int64:
    return len(d)

def countSet[T](set[T] s) -> int64:
    return len(s)

def main():
    # user generic fn over list[T], returning the user generic Optional[T]
    print(firstOf([10, 20, 30]).unwrap())
    print(firstOf(["a", "b"]).unwrap())
    list[int64] empty = []
    print(firstOf(empty).unwrap_or(-1))

    # dict/set type params inferred directly from literal arguments (I#170)
    print(countDict({"x": 1, "y": 2, "z": 3}))
    print(countSet({4, 5, 6, 6}))

    # a user generic instantiated with a collection type argument
    Box[list[int64]] b = Box([7, 8, 9])
    print(b.value[1])

    # a collection of user-generic instances
    list[Pair[int64, str]] ps = [Pair(1, "one"), Pair(2, "two")]
    print(ps[1].first)
    print(ps[1].second)

    # a generic fn over a list of user-generic instances
    print(firstOf(ps).unwrap().second)
