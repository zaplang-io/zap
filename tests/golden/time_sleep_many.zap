# Golden fixture (#1360): many coroutines sleeping concurrently all park on the
# fd-less scheduler timer and wake — without consuming a file descriptor each.
# The park is a registry entry, not a pipe/eventfd, so hundreds of simultaneous
# sleepers cannot exhaust file descriptors (the reason an fd-per-sleep park is
# not the general mechanism). Every sleeper must wake and run its continuation,
# so the final count is exact and deterministic regardless of interleaving.
import time

def napper(Atomic[int64] woken, channel[int64] done):
    time.sleep_for(time.millis(10))
    woken.add(1)
    done <- 1

def main():
    Atomic[int64] woken = Atomic(0)
    auto done = channel[int64](256)
    auto n = 200
    scope:
        auto i = 0
        while i < n:
            spawn napper(woken, done)
            i = i + 1
        auto got = 0
        while got < n:
            auto d = <-done
            got = got + 1
    print("all slept:", woken.load())
