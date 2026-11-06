# Async preemption vs. Boehm stop-the-world overlap regression (#693, D-0119).
#
# #693 is a latent TOCTOU on the gc-boehm async-preemption path: the SIGURG
# preempt handler does an in-signal coroutine switch (`copreempt`) gated by the
# `g_boehm_in_stw` flag, and a worker can begin that switch just as bdwgc begins
# a signal-based stop-the-world — on a weak-memory arch a plain `volatile` read
# of the flag could even observe a stale 0 after STW had begun. The flag is now a
# seq-cst `__atomic` with a release store around the STW bracket and acquire loads
# in the handlers (#213 for this flag), so the ordering half of the race is
# closed; the residual switch-during-STW window is GC-root-safe by the D-0117
# invariant (registers snapshot + green stack scanned as heap).
#
# This golden maximizes the overlap the bug lives in so a regression has a real
# chance to crash or hang the required gate:
#
#   * SPINNERS — tight, allocation-free, yield-free CPU loops (only an atomic load
#     + an integer bump) that the fairness monitor must async-preempt off their M
#     (under Boehm via the in-signal `copreempt`; under precise Immix at the loop
#     back-edge safepoint). With MAXPROCS=2 (see the .env) and more coroutines than
#     Ms, preemption is forced, not incidental.
#   * ALLOCATORS — workers that churn a fresh garbage list every iteration, keep a
#     sparse survivor subset alive, and force an explicit `gc.collect()` every few
#     thousand iterations, driving frequent stop-the-world collections CONCURRENTLY
#     with the spinners being preempted — i.e. SIGURG lands on a worker again and
#     again while the collector is stopping the world.
#
# Each allocator sums its own kept list and sends the total on a channel; the
# per-worker accumulator is order-independent, so the grand total is the UNIQUE
# legal result under every scheduler interleaving and every collection timing — no
# interleaving, timing, or heap-byte value is asserted (GC-golden determinism
# rule). A dropped/stale pointer (a root-visibility fault from an ill-timed
# in-signal switch during STW) would corrupt a worker's sum; a lock-ordering or
# suspend-ordering regression would hang (caught by the harness' 60s timeout).
# Identical and green under moving Immix and non-moving Boehm.
#
# Deterministic total: 4 allocators, reps=15000, a survivor kept every 50th
# iteration carrying value (i + id). For allocator id in 0..3, the kept indices
# are i = 0, 50, 100, ... , 14950 (300 survivors each), so its sum is
#   sum(i for i in {0,50,...,14950}) + 300*id
#   = 50 * (0+1+...+299) + 300*id = 50 * 44850 + 300*id = 2242500 + 300*id.
# Grand total over id=0..3 = 4*2242500 + 300*(0+1+2+3) = 8970000 + 1800 = 8971800.

def spinner(Atomic[bool] done):
    auto spinning = True
    auto x = 0
    while spinning:
        if done.load():
            spinning = False
        x = x + 1

def allocator(channel[int64] ch, int64 id, int64 reps):
    list[int64] keep = []
    for i in range(0, reps):
        auto garbage = [i, i + 1, i + 2]
        if i % 50 == 0:
            keep.append(i + id)
        if i % 3000 == 0:
            gc.collect()
    auto acc = 0
    for v in keep:
        acc = acc + v
    ch <- acc

def main():
    Atomic[bool] done = Atomic(False)
    auto ch = channel[int64]()
    # Spawn the CPU hogs FIRST so, under oversubscription, they are the coroutines
    # that would starve the allocators (and stall collection) without preemption.
    auto s = 0
    while s < 4:
        spawn spinner(done)
        s = s + 1
    auto n = 0
    while n < 4:
        spawn allocator(ch, n, 15000)
        n = n + 1
    auto total = 0
    auto k = 0
    while k < 4:
        total = total + <-ch
        k = k + 1
    done.store(True)  # release the spinners once every allocator has finished
    print(total)
