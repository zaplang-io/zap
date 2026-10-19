# Golden fixture: signals — self-pipe + netpoller + dispatch + coalescing
# (ticket #77, D-0058, design/0005-concurrency.md "Signals").
#
# The full `signal.notify` / `@on_signal` / `scope(cancel_on=..)` surface is
# exercised by other goldens; this one drives the low-level runtime mechanism
# end-to-end behind the internal `__signal_selftest()` hook (mirroring the
# netpoller's `__netpoll_selftest()`). The self-test:
#
#   1. subscribes to SIGUSR2 via the same path `signal.notify` uses — a cap-1
#      coalescing channel wired to the dedicated signal thread, whose handler
#      writes a self-pipe registered with the netpoller (I#80); the signal
#      thread parks on the netpoller and dispatches on readiness;
#   2. raises SIGUSR2 twice: the first is delivered to the cap-1 channel, the
#      second COALESCES (the buffer is full), proving "signals are edges, not a
#      queue" (D-0058);
#   3. receives exactly one delivery of the right value and confirms the channel
#      is then empty (the second edge was dropped).
#
# Returns the number of sub-checks that passed (2 on full success), so the output
# is deterministic. Runs identically under both collectors (Boehm + Immix).

def main():
    print(__signal_selftest())
