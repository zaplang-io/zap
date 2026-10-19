# Golden fixture: signals — `scope(cancel_on=...)` graceful shutdown
# (ticket #77, D-0058, design/0005-concurrency.md "Signals").
#
# The scope is armed to cancel its tree when SIGUSR1 arrives. A long-running
# child (`spin`) cooperates via `checkpoint()`. Inside the block we raise
# SIGUSR1 and `__signal_drain()` until the signal thread has applied the cancel
# (so the scope's `cancelled` flag is set before the dedent joins). The scope
# then awaits `spin`, whose next `checkpoint()` observes the cancel and raises
# `Cancelled` — unwinding it without printing "spin finished". `Cancelled` is
# not a failure, so the scope exits normally and control reaches "shutdown".
# A hard-exit `grace` deadline (ms) backstops a child that refuses to stop; it
# does not fire here because cancellation completes well within it.

def spin():
    auto i = 0
    while i < 1000000000:
        checkpoint()
        i = i + 1
    print("spin finished")

def main() -> int64:
    print("start")
    scope(cancel_on=[Signal.SIGUSR1], grace=5000):
        spawn spin()
        __raise_signal(10)
        __signal_drain()
    print("shutdown")
    return 0
