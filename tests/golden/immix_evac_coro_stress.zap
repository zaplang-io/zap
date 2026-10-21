# GC evacuation under concurrent load (ticket #120). Four coroutines allocate
# heavily and in parallel, each keeping a sparse subset of its allocations alive
# (every 50th) while churning small list garbage, so the heap fragments as every
# worker keeps mutating. Explicit collections fire WHILE all four are active, so
# evacuation relocates survivor buffers out of sparse blocks concurrently with
# ongoing allocation, and the moving collector must rewrite every reference — on
# each coroutine's own shadow stack (reached through its pinned green stack) and
# in every live container — with the world stopped at the cooperative safepoint.
# Each worker then sums its kept list, whose int64 data buffer may itself have
# been relocated; the per-worker accumulator is order-independent, so the summed
# total is deterministic no matter how the scheduler interleaves the Ps or when a
# collection moves objects out from under a running coroutine. A dropped or stale
# pointer would corrupt a worker's sum. No heap-byte assertions; identical under
# the moving Immix collector and non-moving Boehm.
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
    spawn worker(ch, 0, 20000)
    spawn worker(ch, 1, 20000)
    spawn worker(ch, 2, 20000)
    spawn worker(ch, 3, 20000)
    auto total = 0
    auto k = 0
    while k < 4:
        total = total + <-ch
        k = k + 1
    print(total)
