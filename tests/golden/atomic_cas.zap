# Golden: a compare-and-swap retry loop (I#176) — the lock-free idiom. Each of
# four coroutines increments a shared `Atomic[int64]` `iters` times, but instead
# of `add` it spins `load` / `compare_and_swap(cur, cur + 1)` until its CAS wins,
# re-reading on a lost race. A correct CAS (indivisible, and reporting success
# truthfully) makes every increment land exactly once, so the total is
# `n * iters`; a CAS that lied about success or was not atomic would lose
# updates. Joined by a WaitGroup; deterministic under both collectors.
def worker(Atomic[int64] a, bytes wg, int64 iters):
    for i in range(0, iters):
        auto swapped = False
        while not swapped:
            auto cur = a.load()
            swapped = a.compare_and_swap(cur, cur + 1)
    sync.done(wg)

def main():
    Atomic[int64] a = Atomic(0)
    auto wg = sync.waitgroup()
    auto n = 4
    auto iters = 20000
    for k in range(0, n):
        sync.add(wg, 1)
        spawn worker(a, wg, iters)
    sync.wait(wg)
    print(a.load())
