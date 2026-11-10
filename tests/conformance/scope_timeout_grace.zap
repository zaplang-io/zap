# claim: SCOPE-3
# Scope grace bounds the drain of a child that never cooperates. #825: at the
# grace deadline the still-running child is SEVERED (detached and leaked — never
# force-unwound/freed), the scope raises a catchable `ScopeTimeout`, and the
# process keeps running. It is NOT force-exited: before #825 this exact path
# called `_exit(1)` (whole-process kill), which is now reserved for signal-driven
# shutdown (`scope(cancel_on=...)`). The uncooperative child is a bounded blocking
# sleep (never a cancellation point), not the old `while True`, so the leaked
# orphan finishes on its own and the program exits cleanly.
def uncooperative(channel[int64] ready):
    ready <- 1
    time.sleep_ns(time.millis(400))  # never reaches a cancellation point

def main():
    auto ready = channel[int64](1)
    try:
        scope(timeout=100, grace=50):
            spawn uncooperative(ready)
            auto started = <-ready
    except ScopeTimeout:
        print("severed and survived")
    print("still running")
