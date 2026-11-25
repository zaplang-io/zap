# Golden fixture: explicit D-0048 iterator access — `iter()` / `next()` (#1382
# phase 2, RFC #1417). `iter(x)` obtains an iterator over a value that already
# satisfies the protocol (a generator, a generator expression, or a user type
# with `__iter__`/`__next__`) and is idempotent on an iterator; `next(it)` steps
# it, raising a catchable `StopIteration` at exhaustion, while `next(it, default)`
# returns the default and never raises. Both are built on the comma-ok
# `__next__ -> (T, bool)` internally, so the iteration hot path stays
# exception-free (the StopIteration is constructed only on an explicit `next(it)`
# past the end with no default). Reified per-container iterators (`iter([1,2])`)
# are #1382 phase 3; this fixture stays on protocol iterators.

def gen():
    yield 10
    yield 20
    yield 30

# A user type that is its own iterator: `__iter__ -> self`, comma-ok `__next__`.
struct Count:
    int64 i
    def __iter__(self) -> Count:
        return self
    def __next__(self) -> (int64, bool):
        auto cur = self.i
        if cur >= 3:
            return (0, False)
        self.i = cur + 1
        return (cur, True)

def main():
    # --- iter()/next() over a generator ---
    auto it = iter(gen())
    print(next(it))  # 10
    print(next(it))  # 20
    print(next(it))  # 30
    # Exhausted: the default form returns the fallback and never raises.
    print(next(it, -1))  # -1
    print(next(it, -1))  # -1
    assert next(it, -1) == -1

    # --- next(it) past the end raises a catchable StopIteration ---
    auto it2 = iter(gen())
    print(next(it2))  # 10
    print(next(it2))  # 20
    print(next(it2))  # 30
    bool raised = False
    try:
        auto dead = next(it2)
        print(dead)  # (never reached)
    except StopIteration:
        raised = True
    print(raised)  # True
    assert raised

    # --- iter() is idempotent on an iterator (a live view, not a snapshot) ---
    auto g = gen()
    auto same = iter(g)
    print(next(same))  # 10
    print(next(g))  # 20   (same underlying iterator)
    assert next(g) == 30

    # --- a user iterator type, hand-stepped ---
    auto c = iter(Count(i=0))
    print(next(c))  # 0
    print(next(c))  # 1
    print(next(c))  # 2
    print(next(c, -9))  # -9  (exhausted -> default)
    assert next(c, -9) == -9

    # --- next() directly on a protocol iterator (no iter() needed) ---
    auto d = Count(i=0)
    print(next(d))  # 0
    print(next(d))  # 1
