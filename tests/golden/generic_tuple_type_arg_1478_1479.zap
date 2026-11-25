# Regression for #1478 and #1479: a tuple type used as an explicit generic type
# argument. The monomorphizer could not take a tuple type as a type argument,
# which broke generic calls carrying one in two ways, both shared-root:
#
#  * #1478 — a type argument that IS a tuple, `run[(Optional[bytes],
#    Optional[bytes])]`. The tuple index was split into separate type arguments
#    regardless of the generic's arity, so a one-parameter `T` was bound to only
#    the first element; a `(None, None)` argument then failed IR lowering with
#    `ZAP6001: this expression is not supported by codegen yet (Phase 1 slice)`.
#  * #1479 — a type argument CONTAINING a tuple, `run[dict[bytes, (bytes,
#    bytes)]]`. The nested `(bytes, bytes)` made the whole type argument
#    unresolvable, so the generic method was unrecognized and mis-lowered as a
#    field access, `ZAP6001: no field 'run' on struct`.
#
# Both are fixed by teaching `expr_to_type` about tuple types and gating the
# tuple-index split on the generic's declared parameter count. This exercises
# the two issue reducers plus nested tuple type arguments inside `list` and
# `Optional`, pointer (`bytes`) and scalar (`int64`) option payloads, and a
# two-parameter generic whose index must still split. Unblocks
# zaplang-io/memcached#9.


struct Pool:
    int64 unused

    def pair(self) -> (Optional[bytes], Optional[bytes]):
        return (Optional.Some(b"v"), Optional.Some(b"t"))

    def pairs(self) -> dict[bytes, (bytes, bytes)]:
        dict[bytes, (bytes, bytes)] found = {}
        found[b"k"] = (b"value", b"token")
        return found


struct Router:
    Pool pool

    def run[T](self, T fallback, fn(Pool) -> T action) -> T:
        return action(self.pool)

    # #1478: the explicit `T` is a tuple of options; the `(None, None)` fallback
    # must box each element to its `Optional[bytes]`.
    def pair(self) -> (Optional[bytes], Optional[bytes]):
        return self.run[Optional[bytes], Optional[bytes]](
            (None, None),
            lambda pool: pool.pair(),
        )

    # #1479: the explicit `T` is a dict whose value type is a tuple.
    def pairs(self) -> dict[bytes, (bytes, bytes)]:
        dict[bytes, (bytes, bytes)] empty = {}
        auto found = self.run[dict[bytes, (bytes, bytes)]](
            empty,
            lambda pool: pool.pairs(),
        )
        return found


# A one-parameter generic free function used to pin the tuple type argument in
# argument position directly.
def pick[T](T fallback) -> T:
    return fallback


# A two-parameter generic: its tuple index must still split into two arguments.
def first[A, B](A a, B b) -> A:
    return a


def main():
    auto router = Router(Pool(0))

    # #1478 reducer: the action returns the present pair, so both unwrap.
    auto value, token = router.pair()
    print(value.unwrap(), token.unwrap())  # v t

    # #1479 reducer: the dict-of-tuple value round-trips.
    auto dv, dt = router.pairs()[b"k"]
    print(dv, dt)  # value token

    # #1478 direct: a one-parameter generic bound to a tuple of options, all
    # absent (pointer payload). Both elements are `None`.
    auto miss = pick[Optional[bytes], Optional[bytes]]((None, None))
    auto ma, mb = miss
    print(ma.is_none(), mb.is_none())  # True True

    # A present + absent mix.
    auto mixed = pick[Optional[bytes], Optional[bytes]]((Optional.Some(b"hit"), None))
    auto xa, xb = mixed
    print(xa.unwrap(), xb.is_none())  # hit True

    # Scalar (int64) option payload through a tuple type argument.
    auto nums = pick[Optional[int64], Optional[int64]]((Optional.Some(7), None))
    auto na, nb = nums
    print(na.unwrap(), nb.is_none())  # 7 True

    # Nested tuple inside a `list` type argument (one-parameter generic).
    list[(bytes, bytes)] seed = []
    auto xs = pick[list[bytes, bytes]](seed)
    xs.append((b"a", b"b"))
    auto la, lb = xs[0]
    print(la, lb)  # a b

    # Nested tuple inside an `Optional` type argument.
    auto opt = pick[Optional[bytes, bytes]](Optional.Some((b"p", b"q")))
    auto oa, ob = opt.unwrap()
    print(oa, ob)  # p q

    # The two-parameter generic still splits its index into two arguments.
    print(first[int64, str](11, "x"))  # 11
