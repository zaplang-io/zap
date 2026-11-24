# Golden fixture (#1409): the primary ticker idiom — `select` over the ticker's
# channel and a `done` channel. `done` is never signalled, so only the tick case
# fires; the loop takes exactly two ticks and exits. Deterministic count.
import time

def main():
    auto tk = time.ticker(time.millis(10))
    auto done = channel[int64](1)
    auto n = 0
    while n < 2:
        select:
            case t = <-tk.chan:
                n = n + 1
            case d = <-done:
                n = 999
    tk.stop()
    print("selected ticks:", n)
