# Coroutine free-list liveness under spawn + contention (ticket #188, bug 1).
#
# A finished coroutine is unlinked from the live registry and parked on the
# scheduler's free list to recycle its green stack (a later spawn reuses it
# instead of remapping 128 KiB). Under precise gc-immix that free list must be a
# GC root: otherwise the first collection after a coroutine finishes reclaims the
# pooled coroutine object and UNMAPS its green stack, and the next spawn that pops
# it faults in `makecontext` writing the now-unmapped stack (the SIGSEGV reported
# in #188 under sleep + mutex-contention + mass-spawn). gc-boehm never saw it,
# because it conservatively scans the static free-list pointer.
#
# The repro is made deterministic by waves: each wave spawns many short coroutines
# (that sleep and contend a shared mutex, the reported shape), joins them with a
# WaitGroup so every one is FINISHED and pooled on the free list, then forces a
# full `gc.collect()` — which, without the fix, reclaims those pooled coroutines
# and their stacks — before the next wave respawns and REUSES them. A dangling
# reused stack crashes in `makecontext`; a correct root keeps every pooled
# coroutine (and its stack) alive so all waves complete.
#
# The total (waves * per_wave * 2 workers) is order-independent, so the output is
# deterministic regardless of scheduling. No heap-byte assertions; identical under
# the moving Immix collector and non-moving Boehm.

def sleeper(Atomic[int64] c, bytes wg):
    time.sleep_ns(time.micros(300))
    auto junk = [1, 2, 3]            # a little heap churn to feed the collector
    c.add(1)
    sync.done(wg)

def locker(bytes mu, Atomic[int64] c, bytes wg):
    sync.lock(mu)
    time.sleep_ns(time.micros(100))  # hold the lock across a blocking sleep
    c.add(1)
    sync.unlock(mu)
    sync.done(wg)

def main() -> int64:
    Atomic[int64] c = Atomic(0)
    auto mu = sync.mutex()
    auto waves = 6
    auto per_wave = 20
    auto w = 0
    while w < waves:
        auto wg = sync.waitgroup()
        auto k = 0
        while k < per_wave:
            sync.add(wg, 2)
            spawn sleeper(c, wg)
            spawn locker(mu, c, wg)
            k = k + 1
        sync.wait(wg)               # every coroutine of this wave is now pooled
        gc.collect()                # pre-fix: reclaims the pooled stacks -> next
                                    # wave reuses dangling memory -> SIGSEGV
        w = w + 1
    # 6 waves * 20 * 2 workers = 240.
    print(c.load())
    return 0
