# claim: SCOPE-8
# #825 no-grace default (maintainer ruling): a bare `scope(timeout=T)` with NO
# `grace=` is ALWAYS enforceable. At T it cooperatively cancels and then severs a
# non-draining child IMMEDIATELY (implicit grace=0), raising a catchable
# `ScopeTimeout`. Both pre-#825 no-grace behaviors are gone: the join no longer
# hangs forever, and the process is never force-exited. This program must
# therefore TERMINATE (the 60s harness bound catches a regression to hanging) and
# exit 0 (the default `.exit`, which also proves it never reaches `_exit(1)`).
def wont_drain(channel[int64] ready):
    ready <- 1
    # Wall-clock-bounded busy-wait with NO cancellation point (no sleep /
    # checkpoint / channel), so it never observes the cancel and is severed. Since
    # #1357 a coroutine `time.sleep` is itself a cancellation point and would
    # drain, so it can no longer stand in for a non-draining child here.
    auto deadline = time.monotonic_ns() + 500000000
    while time.monotonic_ns() < deadline:
        auto spin = 0

def main():
    auto ready = channel[int64](1)
    try:
        scope(timeout=100):
            spawn wont_drain(ready)
            auto a = <-ready
    except ScopeTimeout:
        print("timed out without grace")
    print("did not hang")
