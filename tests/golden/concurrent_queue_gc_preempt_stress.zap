# Async-preemption vs. GC stop-the-world regression (#710, D-0125).
#
# The #172 preemption monitor fires SIGURG at a worker M every ~10 ms to yield a
# long-running coroutine. Under gc-boehm that signal must NOT land while a
# collection's stop-the-world is in progress: the handler's copreempt stack-swap
# would move a mutator out from under the collector mid-mark, and the signal
# itself races bdwgc's suspend/ack handshake. The observed failures were a hang
# (`main` spinning in GC_generic_malloc_many while every worker sat in bdwgc's
# suspend handler) and a queue-node use-after-free surfacing as
# `unwrap on an Optional.None value` on the consumer. The monitor now gates the
# signal on `g_boehm_in_stw` (D-0125); this golden drives that interaction.
#
# Four producer coroutines push disjoint, known multisets onto one lock-free
# `concurrent.Queue[int64]` while each forces explicit collections mid-push, so a
# stop-the-world repeatedly overlaps active producers and the draining consumer
# (`main`) — precisely the window SIGURG used to corrupt. The drained total is
# compared to the independently computed expected sum: a lost, duplicated, or
# reclaimed node changes the sum, so the printed `1` is the unique correct
# outcome under every scheduler interleaving and every collection timing, and the
# element count proves no-loss / no-duplication. No interleaving, timing, or
# heap-byte value is asserted — identical under moving Immix and non-moving Boehm.
import concurrent

def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def producer(Queue[int64] q, bytes wg, int64 base, int64 reps):
    for i in range(0, reps):
        q.push(base + i)
        if i % 500 == 0:
            gc.collect()
    sync.done(wg)

def main():
    Queue[int64] q = new_queue()
    auto wg = sync.waitgroup()
    auto nprod = 4
    auto reps = 5000
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
