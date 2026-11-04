# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114). The racy counterpart (`racy_counter.zap`) made clean by making the
# shared cell an `Atomic[int64]` — the recommended fix for a shared counter.
#
# `Atomic` is a built-in `@shareable` type, so the static sharing lint (ZAP5125,
# #664) is silent AND `--race` does NOT instrument the cell at all: an atomic
# cell is an asserted-safe shared location, so an `Atomic`-guarded version is
# clean by construction. Each `add(1)` is indivisible, so the total is exact
# regardless of interleaving.
#
# Expected (detector slice):
#   $ zap run --race tests/race/clean_atomic.zap
#   -> clean (no race report); prints 100000; exit 0.
def bump(Atomic[int64] a, bytes wg, int64 iters):
    for i in range(0, iters):
        a.add(1)
    sync.done(wg)


def main():
    Atomic[int64] a = Atomic(0)
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn bump(a, wg, 50000)
    spawn bump(a, wg, 50000)
    sync.wait(wg)
    print(a.load())
