# Golden fixture (#1414): a `select` with no `default` and no ready case, parked
# inside a nested lexical `scope(timeout=)`, must be woken by that scope's own
# deadline. `child` is spawned under a long-lived outer `scope`; inside it opens a
# 25 ms `scope` around a blocking `select` whose single receive case is on a
# channel nobody ever sends to. Before #1414 the parked `select` registered no
# scope-cancellation waiter at all (unlike a bare `<-ch`, which #1398 wired up),
# so neither the inner deadline nor any cancel could reach it — the `select` ran
# forever (the harness would hit its wall-clock timeout) and its `finally` never
# ran. The inner deadline must now wake the `select` AT ONCE, run the body's
# `finally` EXACTLY once, surface `ScopeTimeout` to `child` (D-0094: a scope's OWN
# deadline canceling its body yields `ScopeTimeout`, not `Cancelled`), and let the
# outer scope join normally far inside its own 3000 ms deadline. The channel is
# never sent to, so a missed wake blocks forever rather than passing. Repeated so
# a one-off interleaving cannot pass by luck.
import time

def child(
    channel[int64] ch,
    Atomic[int64] woke,
    Atomic[int64] cleaned,
    Atomic[int64] caught,
):
    try:
        scope(timeout=25):
            try:
                select:
                    case v = <-ch:
                        woke.add(1)  # unreachable: nothing is ever sent on ch
            finally:
                cleaned.add(1)
    except ScopeTimeout:
        caught.add(1)
    except Cancelled:
        # Would mean the inner deadline leaked through as a raw Cancelled instead
        # of being re-labelled ScopeTimeout by the inner scope (D-0094).
        woke.add(1000)

def main():
    auto ch = channel[int64]()
    Atomic[int64] woke = Atomic(0)
    Atomic[int64] cleaned = Atomic(0)
    Atomic[int64] caught = Atomic(0)
    auto i = 0
    while i < 8:
        scope(timeout=3000, grace=500):
            spawn child(ch, woke, cleaned, caught)
        if woke.load() != 0:
            raise ValueError("the select was not cancelled by the nested deadline")
        if cleaned.load() != i + 1:
            raise ValueError("the body cleanup did not run exactly once")
        if caught.load() != i + 1:
            raise ValueError("child did not catch ScopeTimeout from the nested scope")
        i = i + 1
    print("nested scope deadline woke and drained the parked select")
    print("iterations: 8")
