# Golden fixture: `Optional[T]` as a dict key (#704, D-0122). Before the fix,
# an Optional key rejected with `ZAP6001 "Phase 1 slice"`. For CPython parity
# both `None` and a present value are hashable: the Optional routes through the
# synthesized structural `__hash__`/`__eq__` key path, where `None` hashes to a
# fixed sentinel and is equal only to another `None`, and a `Some(v)` delegates
# to `T`'s hashing/equality. Exercises insert, a bare `None` literal key,
# index-read, structural-equality overwrite (a separately built equal `Some`
# hits the same slot), `len`, and membership (`in`) — and confirms `None` and a
# present value never collide or compare equal.
def main():
    dict[Optional[int64], str] d = {}
    Optional[int64] k = Optional.Some(5)
    print(len(d))  # 0
    d[k] = "five"
    d[None] = "none"  # a bare None literal key
    print(len(d))  # 2
    print(d[k])  # five
    print(d[None])  # none

    # Structural overwrite: a separately built equal Some finds the same slot.
    Optional[int64] k2 = Optional.Some(5)
    d[k2] = "FIVE"
    print(len(d))  # 2  (no new key)
    print(d[k])  # FIVE

    # Membership: present Some, present None, absent Some.
    print(k in d)  # True
    print(None in d)  # True
    Optional[int64] k7 = Optional.Some(7)
    print(k7 in d)  # False

    # None and Some occupy distinct slots: overwriting None leaves Some alone.
    d[None] = "NONE"
    print(len(d))  # 2
    print(d[None])  # NONE
    print(d[k])  # FIVE
