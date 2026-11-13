# Golden fixture: async preemption landing in a coroutine's park / exit windows
# (I#889, gc-boehm). Runs with a 250us preemption slice (`.env` sidecar:
# ZAP_PREEMPT_SLICE_US) so the SIGURG preemption signal lands thousands of times
# more often than the default ~10ms slice would allow.
#
# A coroutine that switches away publishes WHY in `g->state` (G_PARKED on a channel
# wait, G_DEAD on exit) and then calls swapcontext; the scheduler dispatches on that
# state when the swap returns. The store -> swap span used to be preemptible: a
# preemption inside it made `copreempt` rewrite the state to G_RUNNABLE, and when the
# scheduler later resumed the coroutine inside the signal handler it set G_RUNNING,
# so the interrupted park/exit sequence then swapped with state G_RUNNING — a state
# `switch_to_g` has no branch for. An exiting coroutine was never reclaimed (the
# program hung at join: g_live never reached 0); a parking one never had its channel
# mutex unlocked nor was woken (every later user of that channel hung).
#
# Each coroutine below computes for well over one slice between switches (so the
# monitor flags it for preemption) and then immediately parks on a channel
# (ping-pong pairs) or exits (a stream of short-lived children), so a good fraction
# of preemptions land right in the window. All totals are order-independent, so the
# output is deterministic however the scheduler interleaves them; before the fix the
# program hangs (the golden harness reports a 60s timeout).

def burn(int64 n) -> int64:
    int64 acc = 0
    for i in range(n):
        acc = acc + i % 7
    return acc

# One half of a ping-pong pair: compute, hand a token over, park until it returns.
def ping(int64 rounds, int64 work, channel[int64] to, channel[int64] back, channel[int64] done):
    int64 total = 0
    for r in range(rounds):
        total = total + burn(work) - burn(work) + 1
        to <- 1
        total = total + <-back
    done <- total

def pong(int64 rounds, int64 work, channel[int64] to, channel[int64] back):
    for r in range(rounds):
        int64 v = <-to
        int64 pad = burn(work)
        back <- v + pad - pad

# A short-lived coroutine: compute past a slice, then exit (the G_DEAD window).
def child(int64 work, channel[int64] done):
    done <- burn(work) * 0 + 1

def spawner(int64 kids, int64 work, channel[int64] done):
    int64 got = 0
    for k in range(kids):
        auto d = channel[int64](1)
        spawn child(work, d)
        got = got + <-d
    done <- got

def main():
    auto done = channel[int64](16)
    for p in range(4):
        auto to = channel[int64]()
        auto back = channel[int64]()
        spawn pong(300, 40000, to, back)
        spawn ping(300, 40000, to, back, done)
    for s in range(4):
        spawn spawner(300, 40000, done)
    int64 total = 0
    for k in range(8):
        total = total + <-done
    print("total", total)
