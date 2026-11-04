# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114; epic #661). Two coroutines increment a plain, non-`@shareable` mutable
# MODULE GLOBAL with NO synchronization.
#
# Under the light concurrency model (#661) this is legal and compiles; the
# static sharing lint (ZAP5125, #664) already warns at each `counter` access.
# `zap run --race` is the DYNAMIC backstop: it MUST report the data race at run
# time, naming a racing access to `counter` with its source location, and exit
# non-zero.
#
# Expected (detector slice):
#   $ zap run --race tests/race/racy_counter.zap
#   -> a data-race report for `counter` with a source location; non-zero exit.
# Without `--race` it compiles and runs, but the printed total is
# nondeterministic (lost updates) — exactly the bug the detector catches.
int64 counter = 0


def bump(bytes wg, int64 iters):
    for i in range(0, iters):
        counter = counter + 1
    sync.done(wg)


def main():
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn bump(wg, 100000)
    spawn bump(wg, 100000)
    sync.wait(wg)
    print(counter)
