# Golden fixture (#1357): a coroutine `time.sleep` must PARK its G and free its
# M, so a sibling on the SAME worker runs during the sleep — the Go model, not a
# thread-blocking `nanosleep`. Pinned to ONE worker (MAXPROCS=1, see the .env).
# The sleeper announces it is about to sleep (via the unbuffered `entered`
# rendezvous) BEFORE the sibling is spawned, so the sibling is only ever
# scheduled onto the single M while the sleeper is already mid-sleep.
#
# If the sleep frees the M, the sibling prints "sibling ran" during the sleep
# and only THEN does the sleeper wake and print "sleeper woke". If the sleep
# pinned the M (the pre-#1357 blocking nanosleep), the single worker stays busy
# for the whole 100 ms, the sibling cannot run until the sleeper wakes, and the
# two lines come out in the opposite order. The line order is the assertion.
import time

def sleeper(channel[int64] entered):
    entered <- 1
    time.sleep_for(time.millis(100))
    print("sleeper woke")

def sibling():
    print("sibling ran")

def main():
    auto entered = channel[int64]()
    scope:
        spawn sleeper(entered)
        auto ready = <-entered
        spawn sibling()
    print("done")
