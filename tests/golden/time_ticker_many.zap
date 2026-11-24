# Golden fixture (#1409): many concurrent tickers all fire without exhausting
# file descriptors. Each waiter creates its own ticker, takes one tick, and stops
# it; every ticker's firing loop arms fd-less scheduler timers (via time.after),
# so a hundred simultaneous tickers consume no file descriptors. Every waiter
# must observe a tick, so the final count is exact.
import time

def waiter(channel[int64] done):
    auto tk = time.ticker(time.millis(10))
    auto t = <-tk.chan
    tk.stop()
    done <- 1

def main():
    auto n = 100
    auto done = channel[int64](128)
    scope:
        auto i = 0
        while i < n:
            spawn waiter(done)
            i = i + 1
        auto got = 0
        while got < n:
            auto d = <-done
            got = got + 1
    print("all ticked:", n)
