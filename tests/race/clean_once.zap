# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114; slice 3 breadth, #679). `Once` establishes happens-before: the
# winner's `once_end` happens-before every other caller's `once_begin` return.
# Two coroutines race to initialize: exactly one (the winner) runs the
# `once_begin`/`once_end` block and writes the module global `shared`; the loser
# blocks in `once_begin` until `once_end` (or arrives after completion). BOTH
# then read `shared`. The Once edge orders the single initializing write before
# every read — the winner reads its own write in program order, the loser reads
# after acquiring the winner's clock — so there is no race.
#
# This is the guarded init pattern: a lazily-initialized global read concurrently
# by many coroutines is clean precisely because `Once` serializes the write
# before every reader. `result1`/`result2` each have a single writer.
#
# Expected (detector slice):
#   $ zap run --race tests/race/clean_once.zap
#   -> clean (no race report); prints 246; exit 0.
int64 shared = 0
int64 result1 = 0
int64 result2 = 0


def init_shared(bytes once):
    if sync.once_begin(once):
        shared = 123
        sync.once_end(once)


def p1(bytes once, bytes wg):
    init_shared(once)
    result1 = shared
    sync.done(wg)


def p2(bytes once, bytes wg):
    init_shared(once)
    result2 = shared
    sync.done(wg)


def main():
    auto once = sync.once()
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn p1(once, wg)
    spawn p2(once, wg)
    sync.wait(wg)
    print(result1 + result2)
