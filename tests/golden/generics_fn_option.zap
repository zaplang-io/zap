# A generic free function returning Option[T] (I#161): first[T] constructs
# Option.Some(xs[0]) or the bare None, both resolved through the return-type
# expected-type sink (Fork I). Exercised at a scalar payload (int64), a pointer
# payload (str), and the empty (None) case; make_pair[A,B] returns a generic
# struct instance inferred from its arguments.
struct Pair[A, B]:
    A first
    B second

def make_pair[A, B](A a, B b) -> Pair[A, B]:
    return Pair(a, b)

def first[T](list[T] xs) -> Option[T]:
    if len(xs) > 0:
        return Option.Some(xs[0])
    return None

def main():
    list[int64] xs = [10, 20, 30]
    print(first(xs).unwrap())
    list[str] ys = ["a", "b"]
    print(first(ys).unwrap())
    list[int64] empty = []
    print(first(empty).unwrap_or(-1))
    Pair[int64, str] p = make_pair(7, "seven")
    print(p.first)
    print(p.second)
