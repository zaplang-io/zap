# ConcurrentMap under real concurrent inserts (#184, D-0147). Four coroutines
# each insert a disjoint range of keys into ONE shared map (passed by reference),
# so they contend on the lock-free list CAS and on lazy bucket initialization
# across cores while the map resizes. Disjoint keys mean no update races, so the
# exact final count and a full read-back prove no lost or corrupted entries under
# both collectors and the preemptive scheduler. No heap-byte assertions.

def worker(ConcurrentMap[int64, int64] m, bytes wg, int64 base, int64 cnt):
    int64 i = 0
    while i < cnt:
        m.put(base + i, (base + i) * 2)
        i = i + 1
    sync.done(wg)

def main():
    ConcurrentMap[int64, int64] m = new_concurrent_map()
    auto wg = sync.waitgroup()
    int64 nw = 4
    int64 per = 500
    int64 w = 0
    while w < nw:
        sync.add(wg, 1)
        spawn worker(m, wg, w * per, per)
        w = w + 1
    sync.wait(wg)
    print(m.length())  # 2000

    int64 total = nw * per
    int64 bad = 0
    int64 i = 0
    while i < total:
        auto v, ok = m.get(i)
        if not ok or v.unwrap() != i * 2:
            bad = bad + 1
        i = i + 1
    print(bad)  # 0
