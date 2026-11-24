# Golden fixture (#1395): when the work case is already ready, `select` takes it
# and the timer value is simply never received. The work channel is pre-filled,
# so its receive is ready immediately; the timer is armed for an hour, so it is
# not ready. `select` picks the only ready case deterministically. The unreceived
# timer neither leaks nor stalls: its firing coroutine is `spawn detached`, so it
# is terminated when `main` returns rather than joined — the cap-1 buffer would
# also absorb the send harmlessly even if it did fire.
import time

def main():
    auto work = channel[int64](1)
    work <- 7
    auto timer = time.after(time.seconds(3600))
    select:
        case v = <-work:
            print(v)
        case fired = <-timer:
            print("WRONG: timer fired before ready work")
