# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114; slice 3 breadth, #679). A `WaitGroup` join establishes happens-before:
# `done()` happens-before the `wait()` that it releases. A `worker` coroutine
# writes the module global `shared`, then calls `done()` on the inner group; a
# `collector` coroutine `wait()`s on that inner group, then reads `shared`. The
# join orders the worker's write before the collector's read — no race.
#
# The joiner here is a SPAWNED coroutine (not the main thread), so the done->wait
# edge is genuinely exercised: unlike the pre-slice-3 clean fixtures (whose
# `main` reads after a join and are clean only because the main thread is
# untracked), this would be flagged without the `WaitGroup` edge. An outer group
# joins both coroutines back to `main`. `result` has a single writer.
#
# Expected (detector slice):
#   $ zap run --race tests/race/clean_waitgroup.zap
#   -> clean (no race report); prints 77; exit 0.
int64 shared = 0
int64 result = 0


def worker(bytes wg_inner):
    shared = 77
    sync.done(wg_inner)


def collector(bytes wg_inner, bytes wg_outer):
    sync.wait(wg_inner)
    result = shared
    sync.done(wg_outer)


def main():
    auto wg_inner = sync.waitgroup()
    auto wg_outer = sync.waitgroup()
    sync.add(wg_inner, 1)
    sync.add(wg_outer, 1)
    spawn worker(wg_inner)
    spawn collector(wg_inner, wg_outer)
    sync.wait(wg_outer)
    print(result)
