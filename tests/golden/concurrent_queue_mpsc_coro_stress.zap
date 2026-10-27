# MPSC queue correctness under concurrent load (ticket #184, D-0078). Four
# producer coroutines each push a known, disjoint multiset onto one lock-free
# `concurrent.Queue[int64]`; the single consumer (`main`) drains exactly
# `nprod*reps` elements. The drained total is compared to the independently
# computed expected sum: a lost or duplicated element would change the sum, so a
# match (printed `1`) is the unique deterministic-correct outcome no matter how
# the scheduler interleaves the producers with the consumer, and the element
# count proves no-loss / no-duplication. `push` is multi-producer (an atomic
# tail swap + a publish store); `pop` is single-consumer. No heap-byte / timing
# assertions — identical under the moving Immix collector and non-moving Boehm.
import concurrent

def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def producer(Queue[int64] q, bytes wg, int64 base, int64 reps):
    for i in range(0, reps):
        q.push(base + i)
    sync.done(wg)

def main():
    Queue[int64] q = new_queue()
    auto wg = sync.waitgroup()
    auto nprod = 4
    auto reps = 20000
    for p in range(0, nprod):
        sync.add(wg, 1)
        spawn producer(q, wg, p * 1000000, reps)
    auto want = nprod * reps
    auto got = 0
    auto total = 0
    while got < want:
        auto v, ok = q.pop()
        if ok:
            total = total + v.unwrap()
            got = got + 1
    sync.wait(wg)
    auto expect = 0
    for p in range(0, nprod):
        for i in range(0, reps):
            expect = expect + (p * 1000000 + i)
    print(b2i(total == expect))
    print(got)
