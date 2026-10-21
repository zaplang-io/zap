# GC + scheduler stress for lock-free per-P bump allocation (I#119). Four
# coroutines allocate concurrently and heavily: each iteration churns a small
# short-lived list (small-object line allocator) AND a 64-element list whose
# 512-byte data buffer exercises the medium-object OVERFLOW allocator, far
# exceeding the collector threshold so many P-local buffer refills and several
# stop-the-world collections run WHILE all four coroutines keep allocating. The
# per-worker accumulator is order-independent, so the summed total is
# deterministic regardless of how the scheduler interleaves the Ps or when a
# collection flushes the P-local bump buffers — a correctness smoke test that
# the lock-free fast path, the slow-path refill, and the STW buffer-flush never
# drop, double-hand-out, or corrupt a live object. Runs identically under the
# precise Immix GC (per-P buffers) and Boehm (which ignores them). NO exact
# heap-byte assertion (those are non-deterministic); only the deterministic sum.
def worker(channel[int64] ch, int64 reps):
    auto acc = 0
    for i in range(0, reps):
        auto xs = [i, i + 1, i + 2]
        acc = acc + xs[0] + xs[1] + xs[2]
        auto big = [k for k in range(0, 64)]
        acc = acc + big[63]
    ch <- acc

def main():
    auto ch = channel[int64]()
    spawn worker(ch, 20000)
    spawn worker(ch, 20000)
    spawn worker(ch, 20000)
    spawn worker(ch, 20000)
    auto total = 0
    auto k = 0
    while k < 4:
        total = total + <-ch
        k = k + 1
    print(total)
