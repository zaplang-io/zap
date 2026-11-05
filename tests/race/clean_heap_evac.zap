# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114) — slice 4 (#684), the RELOCATION path. A shared heap `list` is mutated
# under a `Mutex` by two coroutines WHILE the heap fragments and explicit
# collections fire, so the precise-Immix collector evacuates survivor buffers
# (ticket #120) out from under running coroutines — including, potentially, the
# shared list's own `int64` backing buffer — while the detector holds live
# shadow state for `shared[0]`.
#
# This is the fixture the slice-4 "Done when" asks for: an evacuating-Immix run
# with a shadowed object live across a GC. The detector keys heap shadow on the
# stable list HEADER identity (not the moving buffer address) and the evacuator
# calls `zap_race_on_move` for every relocated object, so the shadow survives the
# move with no aliasing or loss. The `Mutex` orders every access, so the run is
# CLEAN — no false positive — and the total (2 * 20000) is deterministic no
# matter how the scheduler interleaves the Ps or when a collection moves memory.
# Identical under the moving Immix collector and non-moving Boehm; no heap-byte,
# address, or timing assertions (GC-golden rule).
#
# Expected (slice 4):
#   $ zap run --race tests/race/clean_heap_evac.zap
#   -> clean (no race report); prints 40000; exit 0.
def bump(list[int64] shared, bytes mu, bytes wg, int64 iters):
    for i in range(0, iters):
        # Churn small list garbage so blocks fragment and become evacuation
        # sources, then guard the shared-element update with the shared lock.
        auto garbage = [i, i + 1, i + 2]
        sync.lock(mu)
        shared[0] = shared[0] + 1
        sync.unlock(mu)
        if i % 4000 == 0:
            gc.collect()
    sync.done(wg)


def main():
    list[int64] shared = [0]
    auto mu = sync.mutex()
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn bump(shared, mu, wg, 20000)
    spawn bump(shared, mu, wg, 20000)
    sync.wait(wg)
    print(shared[0])
