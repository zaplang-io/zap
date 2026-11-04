# Golden fixture: transparent type aliases end-to-end (#606, D-0092).
#
# A plain alias (`Count = int64`) and two generic aliases — one to a tuple
# (`Pair[T] = (T, T)`) and one to a user generic struct (`Boxed[T] = Box[T]`) —
# all resolve to their targets with the SAME identity (an alias is not a new
# nominal type). The generic aliases participate in monomorphization alongside
# the D-0017 generics: `Pair[int64]` resolves to `(int64, int64)` and
# `Boxed[int64]` to the `Box[int64]` instance. An alias and its spelled-out type
# are therefore interchangeable in signatures and collections.
struct Box[T]:
    T value
    def get(self) -> T:
        return self.value

type Count = int64
type Pair[T] = (T, T)
type Boxed[T] = Box[T]

def double(Count n) -> Count:
    return n + n

def mk_pair(Count a, Count b) -> Pair[int64]:
    return (a, b)

def main():
    Count c = 21
    print(double(c))

    auto p = mk_pair(3, 4)
    print(p[0])
    print(p[1])

    Boxed[int64] b = Box(7)
    print(b.get())

    list[Count] xs = [1, 2, 3]
    Count total = 0
    for x in xs:
        total = total + x
    print(total)

    dict[str, Count] table = {}
    table["k"] = 9
    print(table["k"])
