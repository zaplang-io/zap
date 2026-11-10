# claim: SCOPE-7
# #825: a `scope(timeout=T, grace=G)` whose children never reach a cancellation
# point does NOT force-exit the process (pre-#825 it called `_exit(1)`) and does
# NOT hang. At the grace deadline each still-running child is SEVERED — detached
# and leaked, exactly like `spawn detached`, never force-unwound/freed — and the
# scope raises a catchable `ScopeTimeout` to the caller. Execution continues and
# the process stays alive. Two non-draining kinds are covered together: a blocking
# syscall (`time.sleep_ns`, parked in FFI) and a tight compute loop with no
# safepoint. The leaked orphans are surfaced on stderr (asserted via the `.stderr`
# sidecar). MAXPROCS=4 so both children run concurrently with the main thread.
def sleeper(channel[int64] ready):
    ready <- 1
    time.sleep_ns(time.millis(400))  # blocking syscall: never reaches a cancel point

def spinner(channel[int64] ready, list[int64] stop):
    ready <- 1
    auto i = 0
    # Tight compute loop: no channel op and no checkpoint(), so cooperative
    # cancellation can never land here. `stop[0]` is a bounds-checked heap read
    # (an opaque runtime call, so it observes the parent's later write); the
    # large cap is only a belt-and-braces bound so this can never spin forever.
    while stop[0] == 0 and i < 2000000000:
        i = i + 1

def main():
    auto ready = channel[int64](2)
    auto stop = [0]
    try:
        scope(timeout=100, grace=50):
            spawn sleeper(ready)
            spawn spinner(ready, stop)
            auto a = <-ready
            auto b = <-ready
    except ScopeTimeout:
        print("endpoint timed out")
    # The caller caught ScopeTimeout and keeps running; release the (already
    # severed, leaked) compute orphan so the process exits promptly.
    stop[0] = 1
    print("server still alive")
