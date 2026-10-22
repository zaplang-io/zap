# Golden fixture: a Mutex under real coroutine contention, joined by a
# WaitGroup (ticket #103, D-0032). Four workers each push `iters` elements onto
# one shared deque, serialized by a shared Mutex (deque ops are not internally
# synchronized, so the lock is what keeps the concurrent appends from racing).
# `main` registers the work on a WaitGroup, spawns the fire-and-forget workers,
# and `wait()`s for all four to call `done()` — the D-0032 join pattern. The
# final length is n*iters regardless of interleaving, so the output is
# deterministic while still proving the lock and the join both work (a dropped
# lock would corrupt the deque and change the count; a broken wait would print
# before the workers finished).
def worker(bytes mu, bytes d, bytes wg, int64 iters):
    auto i = 0
    while i < iters:
        sync.lock(mu)
        deque.append(d, 1)
        sync.unlock(mu)
        i = i + 1
    sync.done(wg)

def main():
    auto mu = sync.mutex()
    auto d = deque.new()
    auto wg = sync.waitgroup()
    auto n = 4
    auto iters = 1000
    auto k = 0
    while k < n:
        sync.add(wg, 1)
        spawn worker(mu, d, wg, iters)
        k = k + 1
    sync.wait(wg)
    print(deque.len(d))
