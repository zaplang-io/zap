# Golden fixture: a coroutine that PARKS, is resumed on a different worker
# thread, and then RETURNS from its entry function (I#869).
#
# The M:N scheduler resumes a parked coroutine on whichever worker M picks it up,
# so a coroutine can start on one OS thread and finish on another. The runtime's
# coroutine bootstrap (`g_bootstrap`, zaprt.c) must, once the coroutine's body
# returns, swap back into the scheduler context of the M that is running it NOW —
# not the M it started on. `cur_m` is a `__thread` variable, and at `-O2` on
# aarch64 the compiler keeps the thread-local's *address* in a callee-saved
# register across the whole body (a context switch restores it unchanged), so a
# naive "re-read cur_m" after the body read the OLD thread's M and swapped into a
# scheduler stack another thread was still running on: SIGSEGV /
# "stack smashing detected", ~every run. It passed at `-O0` and on x86_64 only
# because of how those targets address TLS.
#
# Every worker below parks at least once (a blocking channel receive) before it
# returns; many rounds of many coroutines on a multi-worker scheduler make
# cross-thread resumption certain. The totals are order-independent, so the
# output is deterministic however the scheduler interleaves them.

def worker(channel[int64] ch, channel[int64] done):
    int64 v = <-ch
    done <- v

# Parks TWICE (two blocking receives), so it can migrate more than once before it
# finishes.
def relay(channel[int64] a, channel[int64] b, channel[int64] done):
    int64 x = <-a
    int64 y = <-b
    done <- x + y

def main():
    # Phase 1: many single-park coroutines per round, many rounds.
    int64 total = 0
    for round in range(20):
        auto ch = channel[int64]()
        auto done = channel[int64](64)
        scope:
            for i in range(64):
                spawn worker(ch, done)
            for i in range(64):
                ch <- i
        for i in range(64):
            total = total + <-done
    print("single-park total", total)

    # Phase 2: double-park coroutines.
    int64 total2 = 0
    for round in range(10):
        auto a = channel[int64]()
        auto b = channel[int64]()
        auto done2 = channel[int64](32)
        scope:
            for i in range(32):
                spawn relay(a, b, done2)
            for i in range(32):
                a <- i
            for i in range(32):
                b <- 100 + i
        for i in range(32):
            total2 = total2 + <-done2
    print("double-park total", total2)
