# Golden fixture: signals — the default SIGUSR1 handler (ticket #77, D-0058).
#
# With no user subscription for SIGUSR1, the built-in default handler (ON by
# default) runs: it forces a GC and prints a memory-stats summary to stderr.
# `__raise_signal(10)` sends SIGUSR1; `__signal_drain()` blocks until the signal
# thread has dispatched it (so the handler has run and its stderr line is
# present), making the test deterministic. The harness checks that stderr
# contains the handler's banner (signal_default_usr1.stderr).

def main() -> int64:
    __raise_signal(10)
    __signal_drain()
    print("usr1 default ran")
    return 0
