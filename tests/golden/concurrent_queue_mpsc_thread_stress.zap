# MPSC queue under genuine OS-thread parallelism (ticket #184, D-0078). Same
# invariant as concurrent_queue_mpsc_coro_stress, but the producers run on real
# OS threads (`threading.Thread`, I#175) rather than coroutines, so the pushes
# contend across cores (no cooperative interleaving to hide a race). The single
# consumer is `main`. Every producer pushes the SAME range [0, reps), so the
# correct drained total is exactly `nprod * sum[0,reps)` and the count is
# `nprod*reps`: losing an element lowers the total/count, inventing a duplicate
# raises it. (The lambda captures only the loop-invariant `q`/`reps`, so there is
# no loop-variable capture to reason about.) No heap-byte / timing assertions;
# identical under both collectors.
import concurrent

def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def producer(Queue[int64] q, int64 reps):
    for i in range(0, reps):
        q.push(i)

def main():
    Queue[int64] q = new_queue()
    auto nprod = 4
    auto reps = 15000
    list[Thread] ts = []
    for p in range(0, nprod):
        ts.append(threading.Thread(target=lambda: producer(q, reps)))
    for t in ts:
        t.start()
    auto want = nprod * reps
    auto got = 0
    auto total = 0
    while got < want:
        auto v, ok = q.pop()
        if ok:
            total = total + v.unwrap()
            got = got + 1
    for t in ts:
        t.join()
    auto one_pass = 0
    for i in range(0, reps):
        one_pass = one_pass + i
    print(b2i(total == nprod * one_pass))
    print(got)
