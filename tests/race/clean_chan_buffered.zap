# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114; slice 3 breadth, #679). A BUFFERED channel hand-off establishes
# happens-before through the ring buffer: the `producer` writes the module
# global `shared` ONCE, then sends 1000 tokens through a capacity-8 channel (the
# ring slots are reused ~125 times, exercising the per-slot store/load edge); the
# `consumer` drains all 1000, then reads `shared`. Each send of a value
# happens-before the matching receive, so the single write to `shared` (which
# precedes every send in program order) happens-before the consumer's read of it
# (which follows every receive) — no race.
#
# Both accesses to `shared` are in spawned coroutines, so the buffered edge is
# genuinely exercised (the main thread is untracked this slice). `result` has a
# single writer. The total is deterministic (`shared` 99 + last token 999).
#
# Expected (detector slice):
#   $ zap run --race tests/race/clean_chan_buffered.zap
#   -> clean (no race report); prints 1098; exit 0.
int64 shared = 0
int64 result = 0


def producer(channel[int64] ch, bytes wg):
    shared = 99
    for i in range(0, 1000):
        ch <- i
    sync.done(wg)


def consumer(channel[int64] ch, bytes wg):
    auto last = 0
    for i in range(0, 1000):
        last = <-ch
    result = shared + last
    sync.done(wg)


def main():
    auto ch = channel[int64](8)
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn producer(ch, wg)
    spawn consumer(ch, wg)
    sync.wait(wg)
    print(result)
