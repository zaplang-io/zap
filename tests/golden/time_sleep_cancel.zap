# Golden fixture (#1357): a coroutine asleep in `time.sleep_for` inside a
# `scope:` is cancelled cooperatively when a sibling fails. The sleeper parks on
# the netpoller timer (freeing its M) with a one-HOUR deadline; the cancel must
# wake it AT ONCE — not an hour later — raise `Cancelled`, and run its `finally`
# cleanup before the scope exits. The failing sibling waits until the sleeper is
# parked (via the unbuffered `entered` rendezvous) so the cancel always lands at
# or during the wait, making the counts deterministic. If the sleep did not
# observe cancellation the scope could not exit until the hour elapsed and the
# harness would fail on its timeout. Repeated so a one-off interleaving cannot
# pass by luck.
import time

def sleeper(channel[int64] entered, Atomic[int64] cancelled, Atomic[int64] cleaned):
    try:
        entered <- 1
        time.sleep_for(time.seconds(3600))
        print("unreachable: sleeper woke normally")
    except Cancelled:
        cancelled.add(1)
    finally:
        cleaned.add(1)

def fail_after(channel[int64] entered):
    auto ready = <-entered
    raise ValueError("cancel the sleeper")

def main():
    Atomic[int64] cancelled = Atomic(0)
    Atomic[int64] cleaned = Atomic(0)
    auto i = 0
    while i < 16:
        auto entered = channel[int64]()
        auto failed = False
        try:
            scope:
                spawn sleeper(entered, cancelled, cleaned)
                spawn fail_after(entered)
        except ValueError:
            failed = True
        if not failed or cancelled.load() != i + 1 or cleaned.load() != i + 1:
            raise ValueError("sleeping child did not cancel+clean before scope exit")
        i = i + 1
    print("cancelled sleeps cleaned before scope exit")
    print("iterations: 16")
