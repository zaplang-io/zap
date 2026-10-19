# Golden fixture: signals — `signal.notify` delivery (ticket #77, D-0058).
#
# `signal.notify(Signal.SIGUSR1)` takes over SIGUSR1's disposition and returns a
# cap-1 coalescing channel[Signal]. The internal `__raise_signal(10)` test hook
# sends SIGUSR1 to the process; the dedicated signal thread's handler writes the
# self-pipe (registered with the netpoller, I#80), the signal thread dispatches,
# and the value is delivered to the channel. The receive blocks until delivery,
# so the output is deterministic. `int64(sig)` is the signal number (10 = USR1).
#
# Coalescing (a second edge dropped on a full cap-1 channel) is covered by
# `signal_selftest.zap`; this exercises the end-to-end language surface.

def main() -> int64:
    auto ch = signal.notify(Signal.SIGUSR1)
    __raise_signal(10)
    auto sig = <-ch
    print(int64(sig))
    return 0
