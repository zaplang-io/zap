# concurrent.Counter under concurrent increments (ticket #184, D-0078). Four
# coroutines each do `iters` atomic `add(1)` on one shared lock-free
# `concurrent.Counter`, joined by a WaitGroup. The counter is a thin wrapper over
# `Atomic[int64]`, whose `add` is an indivisible seq_cst `atomicrmw`, so the
# final value is `n * iters` regardless of interleaving — a non-atomic
# load/add/store would race and drop updates, changing the count. The shared
# Counter is passed by reference (a heap struct), so all four workers hit the
# same cell. Deterministic under both collectors; no heap-byte assertions.
import concurrent

def worker(Counter c, bytes wg, int64 iters):
    for i in range(0, iters):
        c.add(1)
    sync.done(wg)

def main():
    Counter c = concurrent.Counter()
    auto wg = sync.waitgroup()
    auto n = 4
    auto iters = 50000
    for k in range(0, n):
        sync.add(wg, 1)
        spawn worker(c, wg, iters)
    sync.wait(wg)
    print(c.get())
