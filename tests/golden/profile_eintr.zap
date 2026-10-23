# Golden (ticket #78): the CPU profiler must not cause spurious errors when its
# SIGPROF samples interrupt a workload that mixes on-CPU compute, OS-thread locks
# (runtime.lock_os_thread), and blocking sleeps. profile_eintr.env sets
# ZAP_PROFILE=cpu with a high ZAP_PROFILE_HZ so the sampler storms the shared
# worker Ms while the `cpu` coroutines run (and are sampled) and the `pinned_io`
# coroutines hold a dedicated OS thread across a blocking nanosleep. SIGPROF
# interrupting a blocking syscall (EINTR) must be transparently restarted
# (SA_RESTART) or EINTR-looped; a dropped lock, a lost wakeup, or an unrestarted
# syscall would change the total or crash. The deterministic total (12) and
# exit 0 confirm none of that happened; the harness also checks the end-of-run
# banner on stderr. No sample-count/timing assertion (sampling is
# non-deterministic, like the GC goldens). Green under both collectors.
#
# Note: the blocking sleeps run on `lock_os_thread` (dedicated) Ms on purpose —
# non-pinned `time.sleep_ns` in a tight loop trips a pre-existing gc-immix
# stop-the-world stall unrelated to profiling (reported separately); the
# profiler's own EINTR airtightness (SA_RESTART, the nanosleep rem-loop, the
# poll() fallback loop, and skipping off-CPU coroutines) is independent of it.
def cpu(Atomic[int64] c, bytes wg):
    auto i = 0
    while i < 2000000:
        i = i + 1
    c.add(1)
    sync.done(wg)

def pinned_io(Atomic[int64] c, bytes wg):
    runtime.lock_os_thread()  # OS-thread lock (ticket #172/#175 substrate)
    auto i = 0
    while i < 20:
        time.sleep_ns(time.millis(1))  # blocking nanosleep on the dedicated M
        i = i + 1
    runtime.unlock_os_thread()
    c.add(1)
    sync.done(wg)

def main() -> int64:
    Atomic[int64] c = Atomic(0)
    auto wg = sync.waitgroup()
    auto k = 0
    while k < 6:
        sync.add(wg, 1)
        spawn cpu(c, wg)
        k = k + 1
    auto p = 0
    while p < 6:
        sync.add(wg, 1)
        spawn pinned_io(c, wg)
        p = p + 1
    sync.wait(wg)
    print(c.load())
    return 0
