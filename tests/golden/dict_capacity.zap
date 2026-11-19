# Golden fixture (#1083): dict initial-capacity hint + reserve(). A pre-sized
# SoA table (#840) must behave identically to an organically grown one — same
# entries, membership, deletion/reuse and iteration — while performing ZERO
# regrows during the insert loop. The internal `__ht_resize_count()` hook reads
# the cumulative dict/set table-regrow counter, so the test asserts a regrow
# DELTA of 0 across the inserts (a deterministic check a wall-clock never could
# be), independent of the dict's unspecified iteration order.
def main():
    # `dict[K,V](capacity=n)` pre-sizes once: the insert loop never regrows.
    dict[int64, int64] a = dict[int64, int64](capacity=1000)
    int64 before_a = __ht_resize_count()
    for i in range(0, 1000):
        a[i] = i
    int64 after_a = __ht_resize_count()
    print(after_a - before_a)  # 0 — zero regrows while filling to the hint
    print(len(a))  # 1000

    # `d.reserve(n)` on a fresh dict is the post-construction form — same result.
    dict[int64, int64] b = {}
    b.reserve(1000)
    int64 before_b = __ht_resize_count()
    for i in range(0, 1000):
        b[i] = i
    int64 after_b = __ht_resize_count()
    print(after_b - before_b)  # 0

    # A hint SMALLER than the eventual size still works — the table regrows past
    # the hint exactly as today, losing nothing.
    dict[int64, int64] c = dict[int64, int64](capacity=4)
    int64 before_c = __ht_resize_count()
    for i in range(0, 1000):
        c[i] = i
    int64 after_c = __ht_resize_count()
    print(after_c - before_c > 0)  # True — regrew past the tiny hint
    print(len(c))  # 1000

    # reserve() on a LIVE table grows it (rehashing existing entries) without
    # losing any, then is a no-op when already large enough.
    dict[int64, int64] d = {}
    for i in range(0, 100):
        d[i] = i * 2
    d.reserve(10000)
    print(len(d))  # 100
    int64 s = 0
    for i in range(0, 100):
        s = s + d[i]
    print(s)  # 2 * (0+..+99) = 9900
    int64 before_noop = __ht_resize_count()
    d.reserve(1)  # already far bigger -> no-op, no regrow
    int64 after_noop = __ht_resize_count()
    print(after_noop - before_noop)  # 0

    # `capacity=0` is the default: nothing is pre-allocated, so the first insert
    # allocates the initial table (one regrow) exactly like a plain `{}`.
    dict[int64, int64] e = dict[int64, int64](capacity=0)
    int64 before_e = __ht_resize_count()
    e[1] = 1
    int64 after_e = __ht_resize_count()
    print(after_e - before_e)  # 1
    print(len(e))  # 1

    # Full correctness on a pre-sized table: membership, delete, slot reuse, and
    # a complete key enumeration.
    dict[int64, int64] f = dict[int64, int64](capacity=50)
    for i in range(0, 50):
        f[i] = i + 1000
    print(len(f))  # 50
    print(25 in f)  # True
    print(999 in f)  # False
    del f[10]
    print(10 in f)  # False
    print(len(f))  # 49
    f[10] = 7
    print(f[10])  # 7
    int64 cnt = 0
    for k in f.keys():
        cnt = cnt + 1
    print(cnt)  # 50
