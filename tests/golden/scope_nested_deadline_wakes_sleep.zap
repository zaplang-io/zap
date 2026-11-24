# Golden fixture (#1398): a NESTED lexical `scope(timeout=)` deadline must wake a
# `time.sleep_for` in its own body. `child` is spawned under a long-lived outer
# `scope`; inside it opens a 25 ms `scope` around a 60 s sleep. Before #1398 the
# sleep bound its timer waiter to the coroutine's SPAWN scope (the outer scope),
# so the inner 25 ms deadline could not select the wait — the sleep ran the full
# hour and the outer 3000 ms scope reported the child still parked. The inner
# deadline must now wake the sleep AT ONCE, run the body's `finally` EXACTLY once,
# surface `ScopeTimeout` to `child` (D-0094: a scope's OWN deadline canceling its
# body yields `ScopeTimeout`, not `Cancelled`), and let the outer scope join
# normally far inside its own 3000 ms deadline. The sleep is one HOUR, so if the
# wake were missed the harness would hit its wall-clock timeout instead of
# passing. Repeated so a one-off interleaving cannot pass by luck.
import time

def child(Atomic[int64] woke, Atomic[int64] cleaned, Atomic[int64] caught):
    try:
        scope(timeout=25):
            try:
                time.sleep_for(time.seconds(3600))
                woke.add(1)  # unreachable: the deadline must cancel the sleep
            finally:
                cleaned.add(1)
    except ScopeTimeout:
        caught.add(1)
    except Cancelled:
        # Would mean the inner deadline leaked through as a raw Cancelled instead
        # of being re-labelled ScopeTimeout by the inner scope (D-0094).
        woke.add(1000)

def main():
    Atomic[int64] woke = Atomic(0)
    Atomic[int64] cleaned = Atomic(0)
    Atomic[int64] caught = Atomic(0)
    auto i = 0
    while i < 8:
        scope(timeout=3000, grace=500):
            spawn child(woke, cleaned, caught)
        if woke.load() != 0:
            raise ValueError("the sleep was not cancelled by the nested deadline")
        if cleaned.load() != i + 1:
            raise ValueError("the body cleanup did not run exactly once")
        if caught.load() != i + 1:
            raise ValueError("child did not catch ScopeTimeout from the nested scope")
        i = i + 1
    print("nested scope deadline woke and drained the sleep")
    print("iterations: 8")
