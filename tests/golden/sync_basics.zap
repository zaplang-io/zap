# Golden fixture: sync primitives, single-threaded fast paths (ticket #103,
# D-0032). Exercises Mutex lock/unlock/trylock, Once begin/end, and a WaitGroup
# whose counter is driven to zero before the wait — all on the main thread, so
# every path is deterministic without relying on scheduler interleaving.
#
# `trylock`/`once_begin` return bool; following the set-membership golden
# convention, booleans are printed via an `if` that emits 1/0 (so the output
# does not depend on how a bare bool renders).
def main():
    auto mu = sync.mutex()
    sync.lock(mu)
    # trylock on a held mutex fails.
    if sync.trylock(mu):
        print(1)
    else:
        print(0)
    sync.unlock(mu)
    # trylock on a free mutex succeeds; release it again.
    if sync.trylock(mu):
        print(1)
    else:
        print(0)
    sync.unlock(mu)

    auto o = sync.once()
    # First caller wins; a second call after once_end is a no-op (returns false).
    if sync.once_begin(o):
        print(1)
        sync.once_end(o)
    else:
        print(0)
    if sync.once_begin(o):
        print(1)
        sync.once_end(o)
    else:
        print(0)

    # A WaitGroup already at zero returns from wait() immediately.
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    sync.done(wg)
    sync.done(wg)
    sync.wait(wg)
    print(99)
