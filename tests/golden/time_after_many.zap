# Golden fixture (#1395): many concurrent `time.after` timers all fire. Each
# waiter arms its own timer and receives from it; every timer is a fd-less
# scheduler-timer park (not a pipe/eventfd), so hundreds of simultaneous timers
# cannot exhaust file descriptors. Every waiter must observe its timer fire, so
# the final count is exact and deterministic regardless of interleaving.
import time

def waiter(Atomic[int64] fired, channel[int64] done):
    auto t = <-time.after(time.millis(10))
    fired.add(1)
    done <- 1

def main():
    Atomic[int64] fired = Atomic(0)
    auto done = channel[int64](256)
    auto n = 200
    scope:
        auto i = 0
        while i < n:
            spawn waiter(fired, done)
            i = i + 1
        auto got = 0
        while got < n:
            auto d = <-done
            got = got + 1
    print("all fired:", fired.load())
