# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114; slice 3 breadth, #679). A channel RENDEZVOUS (unbuffered) establishes
# a happens-before edge: a `producer` coroutine writes the plain, non-`@shareable`
# mutable MODULE GLOBAL `shared`, then sends on an unbuffered channel; a
# `consumer` coroutine receives (the rendezvous), then reads `shared`. The send
# happens-before the receive, so the write happens-before the read — no race,
# even though `shared` is unsynchronized by any lock.
#
# Both the write and the read are in SPAWNED coroutines (the main thread stays
# untracked this slice), so the channel edge is genuinely exercised: without the
# send/recv happens-before edge the detector would flag `shared`. `result` is
# written only by the consumer (single writer, no conflict). The total is
# deterministic, proving the ordering.
#
# Expected (detector slice):
#   $ zap run --race tests/race/clean_chan_rendezvous.zap
#   -> clean (no race report); prints 43; exit 0.
int64 shared = 0
int64 result = 0


def producer(channel[int64] ch, bytes wg):
    shared = 42
    ch <- 1
    sync.done(wg)


def consumer(channel[int64] ch, bytes wg):
    auto tok = <-ch
    result = shared + tok
    sync.done(wg)


def main():
    auto ch = channel[int64]()
    auto wg = sync.waitgroup()
    sync.add(wg, 2)
    spawn producer(ch, wg)
    spawn consumer(ch, wg)
    sync.wait(wg)
    print(result)
