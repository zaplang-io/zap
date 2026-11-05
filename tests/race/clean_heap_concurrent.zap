# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114) — slice 4 (#684). A shared heap object mutated concurrently BUT via a
# `concurrent` collection — clean by construction, like `clean_atomic.zap`.
#
# `concurrent.Counter` is a built-in `@shareable` type (a thin wrapper over
# `Atomic[int64]`, #184/D-0078): an asserted-safe shared location. So slice 4
# does NOT instrument its internal state at all (the sharing exemption that
# keeps `Atomic` clean), and each `add(1)` is an indivisible atomic RMW. Two
# coroutines sharing the one heap Counter with no other synchronization is
# therefore clean, and the total (2 * 50000) is exact regardless of interleaving.
#
# Expected (slice 4):
#   $ zap run --race tests/race/clean_heap_concurrent.zap
#   -> clean (no race report); prints 100000; exit 0.
import concurrent


def bump(Counter c, bytes wg, int64 iters):
    for i in range(0, iters):
        c.add(1)
    sync.done(wg)


def main():
    Counter c = concurrent.Counter()
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn bump(c, wg, 50000)
    spawn bump(c, wg, 50000)
    sync.wait(wg)
    print(c.get())
