# ConcurrentMap resize + lazy bucket init (#184, D-0147). Inserting many keys
# drives several lock-free `size` doublings; each new bucket's sentinel is
# created lazily by splitting its parent, and no entry is ever moved. We then
# verify every key reads back, delete the even keys (tombstones), and confirm
# odds remain while evens are gone — across all the resizes. Deterministic;
# identical under both collectors; no heap-byte assertions.

def main():
    ConcurrentMap[int64, int64] m = new_concurrent_map()
    int64 n = 300
    int64 i = 0
    while i < n:
        m.put(i, i * 10)
        i = i + 1
    print(m.length())  # 300

    int64 bad = 0
    i = 0
    while i < n:
        auto v, ok = m.get(i)
        if not ok or v.unwrap() != i * 10:
            bad = bad + 1
        i = i + 1
    print(bad)  # 0  (all present after the resizes)

    i = 0
    while i < n:
        if i % 2 == 0:
            auto d = m.delete(i)
        i = i + 1
    print(m.length())  # 150

    int64 bad2 = 0
    i = 0
    while i < n:
        auto v2, ok2 = m.get(i)
        if i % 2 == 0:
            if ok2:
                bad2 = bad2 + 1
        else:
            if not ok2 or v2.unwrap() != i * 10:
                bad2 = bad2 + 1
        i = i + 1
    print(bad2)  # 0  (evens gone, odds intact)
