# ConcurrentStack under concurrent pushes (#184, D-0148). Four coroutines each
# push a disjoint range of values onto ONE shared Treiber stack (passed by
# reference), contending on the single atomic head CAS across cores. After they
# join, the main coroutine drains all 2000 items and sums them; a lost or
# duplicated push would change the exact total, proving the single-CAS push is
# indivisible under both collectors and the preemptive scheduler. Pop order
# varies, but the sum is invariant. No heap-byte assertions.

def pusher(ConcurrentStack[int64] st, bytes wg, int64 base, int64 cnt):
    int64 i = 0
    while i < cnt:
        st.push(base + i)
        i = i + 1
    sync.done(wg)

def main():
    ConcurrentStack[int64] st = new_concurrent_stack()
    auto wg = sync.waitgroup()
    int64 nw = 4
    int64 per = 500
    int64 w = 0
    while w < nw:
        sync.add(wg, 1)
        spawn pusher(st, wg, w * per, per)
        w = w + 1
    sync.wait(wg)

    int64 total = nw * per
    int64 sum = 0
    int64 popped = 0
    while popped < total:
        auto v, ok = st.pop()
        if ok:
            sum = sum + v.unwrap()
            popped = popped + 1
    print(popped)  # 2000
    print(sum)  # sum of 0..1999 = 1999000
