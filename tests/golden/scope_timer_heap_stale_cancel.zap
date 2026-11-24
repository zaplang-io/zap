# Golden fixture (#1410): the netpoller deadline min-heap stays correct in the
# presence of STALE entries (lazy deletion). Each iteration cancels a long sleeper
# via a short scope deadline — its 1-hour timer never fires, so its heap entry is
# left behind (not actively removed) and must be reclaimed only when the poller
# later pops it and finds the slot no longer armed with that deadline. Meanwhile a
# short 5 ms timer must keep firing correctly on every iteration, with dozens of
# stale far-future entries sitting in the heap. This exercises: many concurrent
# heap entries, the deadline-equality validation that discards stale roots, and
# correct nearest-deadline selection while stale entries are present.
#
# A missed wake would hang on the one-hour sleep (harness wall-clock timeout); a
# stale entry wrongly waking a live waiter would corrupt the counts. Counts are
# exact and deterministic.
import time

def long_sleeper(Atomic[int64] cancelled, Atomic[int64] woke):
    try:
        time.sleep_for(time.seconds(3600))
        woke.add(1)  # unreachable: the scope deadline cancels this
    except Cancelled:
        cancelled.add(1)

def short_worker(Atomic[int64] fired):
    time.sleep_for(time.millis(5))
    fired.add(1)

def main():
    Atomic[int64] cancelled = Atomic(0)
    Atomic[int64] woke = Atomic(0)
    Atomic[int64] fired = Atomic(0)
    auto i = 0
    while i < 40:
        # Cancel a one-hour sleeper: leaves a far-future stale entry in the heap.
        try:
            scope(timeout=15, grace=2000):
                spawn long_sleeper(cancelled, woke)
        except ScopeTimeout:
            pass
        # A short timer must still fire correctly with the stale entries present.
        scope:
            spawn short_worker(fired)
        if cancelled.load() != i + 1:
            raise ValueError("long sleeper was not cancelled by the scope deadline")
        if woke.load() != 0:
            raise ValueError("a cancelled one-hour sleep woke normally")
        if fired.load() != i + 1:
            raise ValueError(
                "a short timer failed to fire with stale heap entries present",
            )
        i = i + 1
    print("deadline heap stayed correct under stale entries")
    print("iterations: 40")
