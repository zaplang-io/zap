# Golden fixture (#1409): a periodic ticker fires repeatedly. `time.ticker(d)`
# returns a Ticker handle; its detached firing loop delivers one value on
# `tk.chan` every interval. `main` (a native thread) receives exactly three
# ticks, then stops the ticker. The count is deterministic (we receive a fixed
# number), independent of interval timing.
import time

def main():
    auto tk = time.ticker(time.millis(10))
    auto i = 0
    while i < 3:
        auto fired = <-tk.chan
        i = i + 1
    tk.stop()
    print("ticks:", i)
