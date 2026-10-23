# Golden: async preemption of a tight, yield-free CPU loop (ticket #172).
#
# Runs with MAXPROCS=1 (see preemption.env) so there is exactly ONE worker M for
# all coroutines. The `spinner` coroutine is spawned first and runs a tight,
# allocation-free, yield-free loop (`spinning` only an atomic load + an integer
# bump) that never reaches a channel op, allocation, or `checkpoint()` — a v1
# COOPERATIVE scheduler would let it monopolize the single M forever, the 100
# `peer` coroutines would never run, `sync.wait` would block, `done` would never
# be set, and the program would hang (the harness' 60s timeout then fails it).
#
# With preemption the monitor time-slices the spinner off the M, the peers each
# increment the shared atomic counter and finish, `sync.wait` returns, `main`
# sets `done`, and the spinner observes it and exits. The printed total (100) is
# a DETERMINISTIC outcome that is only REACHABLE if preemption works — it does
# not assert any timing or GC-byte count. Green under both collectors: under the
# precise collector the yield happens at the loop's GC safepoint; under Boehm the
# async SIGURG handler switches the spinner off its M directly.

def spinner(Atomic[bool] done):
    auto spinning = True
    auto x = 0
    while spinning:
        if done.load():
            spinning = False
        x = x + 1

def peer(Atomic[int64] counter, bytes wg):
    counter.add(1)
    sync.done(wg)

def main():
    Atomic[int64] counter = Atomic(0)
    Atomic[bool] done = Atomic(False)
    auto wg = sync.waitgroup()
    # Spawn the CPU hog FIRST so, on a single M, it is the coroutine that would
    # starve the others without preemption.
    spawn spinner(done)
    auto n = 100
    for k in range(0, n):
        sync.add(wg, 1)
        spawn peer(counter, wg)
    sync.wait(wg)  # returns only once every peer has run
    done.store(True)  # release the spinner
    print(counter.load())
