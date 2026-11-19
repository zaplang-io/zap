# Golden fixture (#1083): set initial-capacity hint + reserve(), the set twin of
# dict_capacity. A pre-sized set must add, dedup, report membership/len and
# iterate identically to an organically grown one while performing ZERO regrows
# during the add loop (asserted via the `__ht_resize_count()` regrow-delta).
# Order-independent.
def main():
    # `set[T](capacity=n)` pre-sizes once: the add loop never regrows.
    set[int64] a = set[int64](capacity=1000)
    int64 before_a = __ht_resize_count()
    for i in range(0, 1000):
        a.add(i)
    int64 after_a = __ht_resize_count()
    print(after_a - before_a)  # 0
    print(len(a))  # 1000

    # `s.reserve(n)` post-construction form — same zero-regrow result.
    set[int64] b = set[int64]()
    b.reserve(1000)
    int64 before_b = __ht_resize_count()
    for i in range(0, 1000):
        b.add(i)
    int64 after_b = __ht_resize_count()
    print(after_b - before_b)  # 0

    # A hint smaller than the eventual size still works (regrows past the hint).
    set[int64] c = set[int64](capacity=4)
    int64 before_c = __ht_resize_count()
    for i in range(0, 1000):
        c.add(i)
    int64 after_c = __ht_resize_count()
    print(after_c - before_c > 0)  # True
    print(len(c))  # 1000

    # reserve() on a live set grows it without losing members; dedup still holds.
    set[int64] d = set[int64]()
    for i in range(0, 100):
        d.add(i)
    d.reserve(10000)
    for i in range(0, 100):
        d.add(i)  # re-adding existing members changes nothing
    print(len(d))  # 100
    int64 present = 0
    for i in range(0, 100):
        if i in d:
            present = present + 1
    print(present)  # 100
    print(500 in d)  # False

    # Full correctness + complete enumeration on a pre-sized set.
    set[int64] e = set[int64](capacity=50)
    for i in range(0, 50):
        e.add(i)
    print(len(e))  # 50
    print(25 in e)  # True
    int64 cnt = 0
    for x in e:
        cnt = cnt + 1
    print(cnt)  # 50

    # String-keyed parity (exercises the wyhash str path under pre-sizing).
    set[str] g = set[str](capacity=100)
    g.add("alpha")
    g.add("beta")
    g.add("alpha")  # duplicate
    print(len(g))  # 2
    print("beta" in g)  # True
    print("gamma" in g)  # False
