# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114; epic #661) — slice 4 (#684), SHARED HEAP OBJECT coverage.
#
# Two coroutines mutate the SAME element of a shared, plain (non-`@shareable`)
# `list[int64]` with NO synchronization. The list is a heap object passed by
# reference to both coroutines, so both hit the same backing element `[0]`. This
# is the heap analogue of `racy_counter.zap` (which races on a module global):
# the race is NOT on a global, so slices 1-3 would never see it — slice 4's
# heap-element instrumentation is what makes `shared[0]` an observed location.
#
# Under the light concurrency model (#661) this compiles and is legal; `zap run
# --race` MUST report the data race on the list element with a source location
# and exit non-zero (66). Keyed on (list header, index), so the report is stable
# regardless of `MAXPROCS`, the collector, or a backing-buffer relocation.
#
# Expected (slice 4):
#   $ zap run --race tests/race/racy_heap_list.zap
#   -> a data-race report for a `shared heap location` (`list element`); non-zero
#      exit. Without `--race` it compiles and runs (nondeterministic total).
def bump(list[int64] shared, bytes wg, int64 iters):
    for i in range(0, iters):
        shared[0] = shared[0] + 1
    sync.done(wg)


def main():
    list[int64] shared = [0]
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn bump(shared, wg, 100000)
    spawn bump(shared, wg, 100000)
    sync.wait(wg)
    print(shared[0])
