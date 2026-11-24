# Golden fixture (#1395): the canonical `select` + timer timeout idiom. The work
# channel is unbuffered and nobody ever sends on it, so its receive case is never
# ready; the `time.after` case becomes ready at the deadline and is the only
# ready case, so `select` deterministically takes the timeout branch. A timer
# channel is just an ordinary channel, so `select` needs no special support — the
# firing coroutine's single send wakes the parked `select` through the normal
# receiver path.
import time

def main():
    auto work = channel[int64]()
    auto timer = time.after(time.millis(20))
    select:
        case v = <-work:
            print(100 + v)
        case fired = <-timer:
            print("timeout")
