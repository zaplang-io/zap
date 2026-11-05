# Module-level globals of a user GENERIC type (#690, epic #661 / D-0112): the
# global's declared generic type is instantiated and its initializer
# monomorphized, so field reads and method calls resolve to the concrete
# instance. A const and a mutable instance, a reassignment of the mutable one,
# and a second, distinct instantiation (`Box[str]`) all resolve independently.
struct Box[T]:
    T item

    def peek(self) -> T:
        return self.item

const Box[int64] GI = Box(7)
Box[int64] mi = Box(8)
const Box[str] GS = Box("hi")

def main():
    print(GI.item)  # 7  — const generic global field read
    print(GI.peek())  # 7  — const generic global method call
    print(mi.peek())  # 8  — mutable generic global method call
    mi = Box(99)
    print(mi.item)  # 99 — reassignment of a mutable generic global
    print(GS.item)  # hi — a distinct instantiation (Box[str])
