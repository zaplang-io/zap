# Scheduler-switch vs. GC-allocator lock-ordering regression (#223).
#
# Eight coroutines — more than the worker-M count on any CI runner, so they
# migrate between Ms and are context-switched constantly — each allocate heavily
# (a fresh garbage list every iteration) while keeping a sparse survivor subset
# (every 50th) alive, and force an explicit collection every 4000 iterations.
# The point is the interaction the #223 deadlock lived in: a coroutine takes the
# bdwgc allocator lock inside GC_MALLOC while, on other Ms, `switch_to_g` updates
# the GC stack bottom on every single context switch. When that update also took
# the allocator lock, a coroutine descheduled while holding the lock deadlocked
# the whole scheduler (gc-boehm, high-core aarch64). The switch path is now
# lock-free, so heavy concurrent alloc + frequent switching + collections makes
# steady progress to a deterministic total under either collector.
#
# Each worker sums its own kept list; the per-worker accumulator is
# order-independent, so the grand total is the unique legal result under EVERY
# scheduler interleaving and every collection timing — no interleaving, timing,
# or heap-byte value is asserted. A dropped/stale pointer (a reclamation or
# root-visibility fault) would corrupt a worker's sum; a lock-ordering regression
# would hang. Identical under moving Immix and non-moving Boehm.
def worker(channel[int64] ch, int64 id, int64 reps):
    list[int64] keep = []
    for i in range(0, reps):
        auto garbage = [i, i + 1, i + 2]
        if i % 50 == 0:
            keep.append(i + id)
        if i % 4000 == 0:
            gc.collect()
    auto acc = 0
    for v in keep:
        acc = acc + v
    ch <- acc

def main():
    auto ch = channel[int64]()
    auto n = 0
    while n < 8:
        spawn worker(ch, n, 20000)
        n = n + 1
    auto total = 0
    auto k = 0
    while k < 8:
        total = total + <-ch
        k = k + 1
    print(total)
