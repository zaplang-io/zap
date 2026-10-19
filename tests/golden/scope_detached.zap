# Golden fixture: structured concurrency — `spawn detached` opts out of scope
# ownership and outlives the block (ticket #50, D-0044,
# design/0005-concurrency.md "Detached coroutines").
#
# `forever` blocks on a receive that never arrives. If it were an *owned* spawn
# the scope's join barrier would wait for it forever (deadlock). Because it is
# `spawn detached`, the scope owns nothing and exits immediately, and the
# program does not block on the detached coroutine at exit either — so `main`
# proceeds and prints, and the process terminates the still-parked coroutine on
# exit (Go goroutine semantics). The output is deterministic.

def forever(channel[int64] ch):
    auto x = <-ch
    print(x)

def main():
    auto ch = channel[int64]()
    scope:
        spawn detached forever(ch)
    print("scope exited")
