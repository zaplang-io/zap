# Nested generic calls (I#161): a generic fn calls another generic fn, so the
# monomorphization worklist must reach a fixpoint — instantiating wrap[int64]
# and twice[int64] discovers identity[int64]. Proves cross-function
# instantiation discovery and that a generic fn's result type flows into the
# next inference.
struct Pair[A, B]:
    A first
    B second

def identity[T](T x) -> T:
    return x

def wrap[T](T x) -> Optional[T]:
    return Optional.Some(identity(x))

def twice[T](T x) -> Pair[T, T]:
    return Pair(identity(x), x)

def main():
    print(wrap(5).unwrap())
    print(wrap("hi").unwrap())
    Pair[int64, int64] p = twice(8)
    print(p.first)
    print(p.second)
