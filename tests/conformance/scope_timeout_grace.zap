# claim: SCOPE-3
# Scope grace bounds the drain of a child that never cooperates. #825: at the
# grace deadline the still-running child is SEVERED (detached and leaked — never
# force-unwound/freed), the scope raises a catchable `ScopeTimeout`, and the
# process keeps running. It is NOT force-exited: before #825 this exact path
# called `_exit(1)` (whole-process kill), which is now reserved for signal-driven
# shutdown (`scope(cancel_on=...)`). The uncooperative child is a wall-clock-
# bounded busy-wait that reaches NO cancellation point, so the leaked orphan
# finishes on its own and the program exits cleanly. (Since #1357 a coroutine
# `time.sleep` IS a cancellation point — it parks and drains cooperatively — so a
# sleep can no longer stand in for a non-draining child here; the busy-wait polls
# the monotonic clock with no sleep/checkpoint/channel op and so never observes
# the cancel.)
def uncooperative(channel[int64] ready):
    ready <- 1
    auto deadline = time.monotonic_ns() + 400000000
    while time.monotonic_ns() < deadline:
        auto spin = 0

def main():
    auto ready = channel[int64](1)
    try:
        scope(timeout=100, grace=50):
            spawn uncooperative(ready)
            auto started = <-ready
    except ScopeTimeout:
        print("severed and survived")
    print("still running")
