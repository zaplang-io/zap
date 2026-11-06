# Golden fixture: `Optional[T]` as a set element (#704, D-0122). Set elements
# are hashed and compared exactly like dict keys, so an Optional element uses the
# same synthesized structural `__hash__`/`__eq__`: `None` is its own element,
# equal only to another `None`, and a present value dedups structurally.
# Exercises the typed empty constructor, `.add` of both a `Some` and a bare
# `None` (new and already-present), `len`, and membership — confirming `None`
# and a present value never collide.
def main():
    set[Optional[int64]] s = set[Optional[int64]]()
    print(len(s))  # 0
    Optional[int64] k = Optional.Some(5)
    s.add(k)
    s.add(None)
    s.add(Optional.Some(5))  # structural duplicate of k -> no-op
    s.add(None)  # already present -> no-op
    print(len(s))  # 2
    print(k in s)  # True
    print(None in s)  # True
    print(Optional.Some(9) in s)  # False
