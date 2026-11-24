# Golden fixture (#1409): a consumer slower than the interval still makes
# progress and never deadlocks. The ticker fires every 5ms but the consumer
# sleeps 30ms between receives, so most ticks are dropped (the cap-1 channel plus
# non-blocking send keeps the firing loop on schedule instead of blocking on the
# slow consumer). Each receive still returns exactly one buffered tick, so the
# loop completes with a deterministic count.
import time

def main():
    auto tk = time.ticker(time.millis(5))
    auto i = 0
    while i < 3:
        time.sleep_for(time.millis(30))
        auto t = <-tk.chan
        i = i + 1
    tk.stop()
    print("slow consumer got:", i)
