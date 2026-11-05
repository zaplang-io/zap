# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114; epic #661). A WRITE/WRITE race: two coroutines each write a plain,
# non-`@shareable` mutable MODULE GLOBAL with NO synchronization and NO read of
# it (pure stores, `counter = v`), so the conflict is write-vs-write rather than
# the read-modify-write of `racy_counter.zap`.
#
# There is no happens-before edge between the two writers (both are spawned by
# `main`, which creates a parent->child edge to each child but none between the
# siblings), so every pair of their writes to `counter` races. `--race` MUST
# report the data race and exit non-zero, regardless of how the two coroutines
# interleave — the verdict is schedule-independent (the lack of an ordering
# edge is what is reported, not a particular timing).
#
# Expected (detector slice):
#   $ zap run --race tests/race/racy_write_write.zap
#   -> a data-race report for `counter` with a source location; non-zero exit.
int64 counter = 0


def writer(bytes wg, int64 v, int64 iters):
    for i in range(0, iters):
        counter = v
    sync.done(wg)


def main():
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn writer(wg, 1, 100000)
    spawn writer(wg, 2, 100000)
    sync.wait(wg)
    print(counter)
