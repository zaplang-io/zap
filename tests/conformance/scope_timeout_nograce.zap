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
    time.sleep_ns(time.millis(500))  # blocking syscall: never reaches a cancel point

def main():
    auto ready = channel[int64](1)
    try:
        scope(timeout=100):
            spawn wont_drain(ready)
            auto a = <-ready
    except ScopeTimeout:
        print("timed out without grace")
    print("did not hang")
