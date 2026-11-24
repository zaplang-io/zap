# Golden fixture (#1395): `time.after(d)` returns a one-shot timer channel that
# receives exactly one value after the duration — the monotonic-clock instant
# (ns) at which it fired. `main` is a native thread with no M; it block-receives
# on the timer channel while a worker runs the detached firing coroutine, which
# parks on the fd-less scheduler timer and sends once at the deadline. The
# delivered timestamp is taken AFTER the call, so it is >= the pre-call reading.
import time

def main():
    auto before = time.monotonic_ns()
    auto fired = <-time.after(time.millis(20))
    if fired >= before:
        print("fired")
    else:
        print("WRONG: fire instant precedes the call")
