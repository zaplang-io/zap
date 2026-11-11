# AtomicArray[int64] under real concurrent CAS contention (#844). Four coroutines
# each perform `iters` compare-and-swap increments on ONE shared slot, retrying
# on a losing CAS (the canonical lock-free increment loop). The shared array is a
# heap reference passed to every worker, so all four hammer the same cell across
# cores. A single lost or duplicated update changes the total, so the exact
# `n * iters` proves the seq_cst CAS is indivisible under both collectors and the
# preemptive scheduler. No heap-byte assertions (the house rule for GC goldens).

def worker(AtomicArray[int64] a, bytes wg, int64 iters):
    for i in range(0, iters):
        auto done = False
        while not done:
            auto cur = a.load(0)
            done = a.compare_and_swap(0, cur, cur + 1)
    sync.done(wg)

def main():
    AtomicArray[int64] a = AtomicArray[int64](1)
    auto wg = sync.waitgroup()
    auto n = 4
    auto iters = 25000
    for k in range(0, n):
        sync.add(wg, 1)
        spawn worker(a, wg, iters)
    sync.wait(wg)
    print(a.load(0))  # 100000
