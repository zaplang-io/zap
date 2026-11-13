# Golden fixture: async preemption inside an explicit collection (I#888, gc-boehm).
#
# Under the Boehm collector `gc.collect()` is `GC_gcollect()`, which holds the
# collector's allocator lock for the whole collection. The scheduler preempts a
# coroutine that runs too long by signalling its worker thread (SIGURG) and
# switching to another coroutine on the same M — but only when the coroutine is
# not inside a runtime critical section (`no_preempt == 0`). `GC_gcollect` was not
# such a section, so a preemption landing inside it switched away with the
# allocator lock held; the sibling coroutine run next on that same thread then
# blocked in its first allocation on a non-recursive mutex its OWN thread already
# owned: a self-deadlock, and every worker piled up behind it.
#
# Here several coroutines call `gc.collect()` back to back (each collection is long
# enough that a preemption tick lands inside it almost every time) while others
# allocate on the same multi-worker scheduler, so a coroutine switched in while a
# collecting one is parked mid-collection allocates immediately. Every total is
# order-independent, so the output is deterministic however the scheduler
# interleaves them; before the fix the program hangs (the golden harness reports
# a 60s timeout), after it, it finishes in about a second. The `.env` sidecar sets
# ZAP_PREEMPT_SLICE_US=250 (a shorter preemption slice than the default ~10ms) so the
# preemption tick lands inside a collection reliably instead of once in a long while.

def collector(int64 reps, channel[int64] done):
    for i in range(0, reps):
        gc.collect()
    done <- reps

def allocator(int64 id, int64 reps, channel[int64] done):
    int64 acc = 0
    for i in range(0, reps):
        auto garbage = [i, i + 1, i + 2]
        acc = acc + garbage[1] + id
    done <- acc

def main():
    auto done = channel[int64](32)
    for k in range(8):
        spawn collector(1500, done)
    for k in range(8):
        spawn allocator(k, 600000, done)
    int64 total = 0
    for k in range(16):
        total = total + <-done
    print("total", total)
