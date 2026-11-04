# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114). A plain, non-`@shareable` mutable MODULE GLOBAL that is mutated by
# only ONE coroutine — the other coroutine never touches it. There is no
# concurrent access to `counter`, hence no data race, even though its type is
# not `@shareable`.
#
# This guards against a false positive: a happens-before detector must report a
# race only for genuinely concurrent conflicting accesses, not merely because a
# non-`@shareable` global exists. (The static lint ZAP5125 is coarser and warns
# on the type alone; `--race` must stay quiet here.)
#
# Expected (detector slice):
#   $ zap run --race tests/race/clean_single.zap
#   -> clean (no race report); prints 50000; exit 0.
int64 counter = 0


def worker(bytes wg, int64 iters):
    for i in range(0, iters):
        counter = counter + 1
    sync.done(wg)


def other(bytes wg):
    print("other coroutine touches nothing shared")
    sync.done(wg)


def main():
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn worker(wg, 50000)
    spawn other(wg)
    sync.wait(wg)
    print(counter)
