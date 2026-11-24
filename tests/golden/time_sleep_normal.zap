# Golden fixture (#1357): normal (uncancelled) wake. A coroutine in a `scope:`
# sleeps a short time and returns NORMALLY — the scope completes and nothing is
# raised. `main` (a native thread with no owning scope) also sleeps: there is no
# M to free and nothing to cancel, so it takes the precise blocking fallback.
# Both simply return. Deterministic output.
import time

def nap(channel[int64] done):
    time.sleep_for(time.millis(20))
    done <- 1

def main():
    auto start = time.monotonic_ns()
    time.sleep_for(time.millis(10))  # native-thread blocking fallback
    if time.monotonic_ns() - start < 0:
        raise ValueError("monotonic clock went backwards")
    print("main slept")
    auto done = channel[int64]()
    scope:
        spawn nap(done)
        auto got = <-done
    print("coroutine slept")
