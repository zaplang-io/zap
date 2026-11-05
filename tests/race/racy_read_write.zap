# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114; epic #661). A READ/WRITE race: one coroutine ONLY writes a plain,
# non-`@shareable` mutable MODULE GLOBAL (`counter = i`, a pure store) while a
# second coroutine ONLY reads it (`sink = counter`), with NO synchronization
# between them. A write concurrent with a read of the same location is a data
# race just as two writes are.
#
# `sink` is written by only the one reader coroutine, so it never races (a
# single-writer global has no conflicting concurrent access) — this also checks
# the detector does not confuse the two globals. The racing global is `counter`.
# As with the other racy fixtures the verdict is schedule-independent: there is
# no happens-before edge between the writer and the reader, so `--race` MUST
# report the race on `counter` and exit non-zero however they interleave.
#
# Expected (detector slice):
#   $ zap run --race tests/race/racy_read_write.zap
#   -> a data-race report for `counter` with a source location; non-zero exit.
int64 counter = 0
int64 sink = 0


def writer(bytes wg, int64 iters):
    for i in range(0, iters):
        counter = i
    sync.done(wg)


def reader(bytes wg, int64 iters):
    for i in range(0, iters):
        sink = counter
    sync.done(wg)


def main():
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn writer(wg, 100000)
    spawn reader(wg, 100000)
    sync.wait(wg)
    print(sink)
