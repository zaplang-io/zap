# Golden: concurrent atomic increments converge to the EXACT total — no lost
# updates (I#176). Four real coroutines each do `iters` atomic `add(1)` on one
# shared `Atomic[int64]`, joined by a WaitGroup. `atomicrmw add` is indivisible,
# so the final value is `n * iters` regardless of interleaving: a non-atomic
# `load`/`+1`/`store` would race and drop updates, changing the count. The shared
# cell is passed by reference (it is a heap box), so all four workers hit the same
# word. Deterministic under both collectors.
def worker(Atomic[int64] a, bytes wg, int64 iters):
    for i in range(0, iters):
        a.add(1)
    sync.done(wg)

def main():
    Atomic[int64] a = Atomic(0)
    auto wg = sync.waitgroup()
    auto n = 4
    auto iters = 50000
    for k in range(0, n):
        sync.add(wg, 1)
        spawn worker(a, wg, iters)
    sync.wait(wg)
    print(a.load())
