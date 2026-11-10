# Golden: user-defined generic containers with capability bounds (D-0139/I#811).
#
# A user generic container declares a capability bound (`Equatable`/`Hashable`/
# `Comparable`) on a type parameter exactly as the built-in containers do, closing
# the loop so the capability model is uniform across built-in and user containers.
# The bound is enforced at each concrete instantiation (a non-satisfying argument
# is a compile error, exercised in the mono unit tests); here we confirm the
# accepted shapes LOWER AND RUN:
#   * `Comparable` with a scalar key — the body uses `<` on the bound parameter,
#     which lowers per concrete instance (static dispatch, no vtable);
#   * `Comparable` with a value-struct key that `@derive(Ord)`s;
#   * `Hashable` with a scalar element — the body uses `==`;
#   * the user container itself (a value struct, so it auto-derives Eq+Hash from
#     its fields, #808) used as a KEY of a built-in `dict`, demonstrating the two
#     container kinds compose under one capability model.
# GC is unaffected: the heap `str`/struct fields are traced by the existing
# per-struct descriptors — capability bounds add no runtime mechanism.

@derive(Eq, Ord)
value struct Key:
    int64 id


value struct Pair[K: Comparable, V]:
    K key
    V val

    def less(self, K other) -> bool:
        return self.key < other

    def get(self) -> V:
        return self.val


value struct Bag[E: Hashable]:
    E first

    def same(self, E other) -> bool:
        return self.first == other


def main():
    # Comparable bound, scalar key: `<` lowers to an integer compare.
    Pair[int64, str] p = Pair[int64, str](key=5, val="five")
    print(p.get())  # five
    print(p.less(9))  # True
    print(p.less(1))  # False

    # Comparable bound, value-struct key deriving Ord.
    Pair[Key, str] pk = Pair[Key, str](key=Key(id=2), val="k2")
    print(pk.get())  # k2

    # Hashable bound, scalar element: `==` lowers to a string compare.
    Bag[str] b = Bag[str](first="x")
    print(b.same("x"))  # True
    print(b.same("y"))  # False

    # Uniformity: the user container auto-derives Eq+Hash (#808) and so is itself
    # a valid KEY of a built-in dict — equal instances collide to one slot.
    dict[Pair[int64, str], int64] d = {}
    d[Pair[int64, str](key=1, val="a")] = 7
    print(d[Pair[int64, str](key=1, val="a")])  # 7
    print(len(d))  # 1
