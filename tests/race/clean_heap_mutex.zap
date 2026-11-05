# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114) — slice 4 (#684). The racy counterpart (`racy_heap_list.zap`) made
# CLEAN by serializing every access to the shared heap list element with one
# shared `Mutex`.
#
# The lock creates a happens-before edge between every pair of accesses to
# `shared[0]`, so even though slice 4 instruments the element read and write,
# `--race` is PRECISE and reports NOTHING — no false positive on a correctly
# guarded shared heap object. The deterministic total (2 * 50000) proves the
# ordering held.
#
# Expected (slice 4):
#   $ zap run --race tests/race/clean_heap_mutex.zap
#   -> clean (no race report); prints 100000; exit 0.
def bump(list[int64] shared, bytes mu, bytes wg, int64 iters):
    for i in range(0, iters):
        sync.lock(mu)
        shared[0] = shared[0] + 1
        sync.unlock(mu)
    sync.done(wg)


def main():
    list[int64] shared = [0]
    auto mu = sync.mutex()
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn bump(shared, mu, wg, 50000)
    spawn bump(shared, mu, wg, 50000)
    sync.wait(wg)
    print(shared[0])
