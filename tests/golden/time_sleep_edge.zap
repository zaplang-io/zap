# Golden fixture (#1357): edge durations and overflow. Zero / negative /
# already-past sleeps do not hang and, inside a `scope:`, act as cooperative
# checkpoints (observe a pending cancel, else yield once) rather than a silent
# no-op that could starve siblings; nothing is cancelled here, so each returns
# at once. A near-max `int64` nanosecond deadline (~292 years) combined with an
# immediate sibling failure proves the deadline never gates cancellation and
# never overflows to a past deadline that would wake early: the scope exits at
# once via `Cancelled`, not after the duration. Deterministic output.
import time

def zero_naps(channel[int64] done):
    time.sleep_ns(0)
    time.sleep_ns(-1)
    time.sleep_for(time.millis(-5))
    time.sleep(0.0)
    done <- 1

def long_then_cancelled(channel[int64] entered, Atomic[int64] woke):
    try:
        entered <- 1
        time.sleep_ns(9223372036854775807)
    except Cancelled:
        woke.add(1)

def boom(channel[int64] entered):
    auto ready = <-entered
    raise ValueError("cancel the long sleeper")

def main():
    auto done = channel[int64]()
    scope:
        spawn zero_naps(done)
        auto got = <-done
    print("zero/neg returned")
    Atomic[int64] woke = Atomic(0)
    auto entered = channel[int64]()
    auto failed = False
    try:
        scope:
            spawn long_then_cancelled(entered, woke)
            spawn boom(entered)
    except ValueError:
        failed = True
    if not failed or woke.load() != 1:
        raise ValueError("near-max-deadline sleep did not cancel promptly")
    print("huge deadline cancelled promptly")
