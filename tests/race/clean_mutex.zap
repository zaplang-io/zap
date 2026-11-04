# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114). The racy counterpart (`racy_counter.zap`) made clean by serializing
# every access to the shared module global with one shared `Mutex`.
#
# Note the lint/detector relationship: the static sharing lint (ZAP5125, #664)
# is TYPE-based and still warns at the `counter` accesses (it cannot see that a
# separate `Mutex` protects them), but `--race` is PRECISE and reports NOTHING —
# the lock creates a happens-before edge between every pair of accesses, so
# there is no race. The deterministic total proves the ordering.
#
# Expected (detector slice):
#   $ zap run --race tests/race/clean_mutex.zap
#   -> clean (no race report); prints 100000; exit 0.
int64 counter = 0


def bump(bytes mu, bytes wg, int64 iters):
    for i in range(0, iters):
        sync.lock(mu)
        counter = counter + 1
        sync.unlock(mu)
    sync.done(wg)


def main():
    auto mu = sync.mutex()
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn bump(mu, wg, 50000)
    spawn bump(mu, wg, 50000)
    sync.wait(wg)
    print(counter)
