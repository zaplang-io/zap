# Stop-the-world liveness across a blocking sleep (ticket #188, bug 2).
#
# A coroutine blocked in `time.sleep` / `time.sleep_ns` must be marked GC-safe
# (its P handed off via the runtime's enter/leave-blocking bracket) so a
# stop-the-world collection need not wait for it to reach a safepoint. Without
# that bracket a sleeping coroutine stays GC-ACTIVE, so a collection that starts
# while it sleeps blocks until the sleep ends — for a long sleep, effectively
# forever.
#
# This makes the regression deterministic. A DAEMON thread (excluded from every
# program-exit join barrier) parks in a single very long `time.sleep`; `main`
# waits until it is asleep, then forces repeated `gc.collect()`s. With the bug,
# the first collection stop-the-world-waits on the sleeping daemon's dedicated M
# and hangs for the whole sleep (the golden harness then fails it on the 60s
# timeout). With the fix the daemon is GC-safe while asleep, every collection
# completes at once, and `main` prints and returns promptly — the daemon's
# unfinished long sleep does not hold the process open (daemon threads are
# awaited by nothing). Output is a fixed string; no heap-byte / timing
# assertions. Under gc-boehm (threads stopped by signal, not cooperatively) the
# collection never waited on the sleeper, so this is stable there too.

def long_sleeper(Atomic[int64] ready):
    ready.store(1)          # announce we are about to block
    time.sleep(120.0)       # one long blocking sleep on this daemon's dedicated M

def main() -> int64:
    Atomic[int64] ready = Atomic(0)
    auto t = threading.Thread(target=lambda: long_sleeper(ready), daemon=True)
    t.start()
    while ready.load() == 0:   # wait until the daemon has reached its sleep
        auto spin = 0
    # Hammer stop-the-world while the daemon is parked in its long sleep. Pre-fix
    # the first of these blocks until the 120s sleep ends -> harness timeout;
    # post-fix each returns immediately because the daemon is GC-safe.
    auto i = 0
    while i < 200:
        auto junk = [i, i, i]
        gc.collect()
        i = i + 1
    print("stw-ok")
    return 0
