# Golden: SIGUSR2 toggles the CPU profiler (ticket #78, D-0058).
#
# `__raise_signal(12)` sends SIGUSR2; `__signal_drain()` blocks until the signal
# thread has dispatched it, so the toggle has definitely run before the next
# line — deterministic, no timing. A coroutine is spawned FIRST so the scheduler
# (the worker Ms the profiler samples) exists before the start toggle; otherwise
# profiling would decline to start. The first toggle starts profiling, the second
# stops it and writes the folded output plus the end-of-run banner the harness
# matches on stderr. ZAP_PROFILE_OUT=/dev/null (profile_toggle.env) keeps the
# test dir clean. The printed total is the deterministic sentinel; no
# sample-count/timing assertion (sampling is non-deterministic).
def busy(Atomic[int64] c, bytes wg):
    auto i = 0
    while i < 3000000:
        i = i + 1
    c.add(1)
    sync.done(wg)

def main() -> int64:
    Atomic[int64] c = Atomic(0)
    auto wg = sync.waitgroup()
    sync.add(wg, 1)
    spawn busy(c, wg)        # the scheduler (worker Ms) now exists
    __raise_signal(12)       # SIGUSR2 -> start profiling
    __signal_drain()
    sync.wait(wg)            # busy runs on a worker M and is sampled
    __raise_signal(12)       # SIGUSR2 -> stop + flush + banner
    __signal_drain()
    print(c.load())
    return 0
