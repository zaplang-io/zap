# Golden fixture (#1414): the walled garden around the parked-`select`
# cancellation fix. Three shapes the one-level `select_scope_deadline_wakes`
# fixture does not cover, each asserted over several iterations:
#
#   A. THREE-DEEP lexical nesting. Only the INNERMOST scope carries the 25 ms
#      deadline; two enclosing deadline-less scopes wrap the parked `select`. The
#      select must bind to the innermost scope, wake on its deadline, surface
#      `ScopeTimeout`, and let that `ScopeTimeout` propagate out through both
#      enclosing scopes unchanged (they neither swallow nor relabel it).
#
#   B. A MULTI-CASE `select` (two receive cases plus a send case, on three
#      distinct channels, none ever ready) parked straight in a timed scope's
#      body. The deadline must wake it EXACTLY ONCE and drain all three per-case
#      waiters: the fix registers one scope-cancellation waiter per case, so a
#      double wake (claiming the select twice -> re-scheduling the same coroutine)
#      would corrupt the scheduler or crash. Exercises both the recvq and the
#      sendq cancellation paths in one select.
#
#   C. A spawned `select` parked as a SIBLING of a failing coroutine in the same
#      default-OnError.Cancel `scope`. The sibling's failure must cancel and wake
#      the parked select, which observes `Cancelled` and runs its `finally`; the
#      scope then re-raises the one real failure. Proves cancellation reaches a
#      parked select from a sibling failure, not only from a deadline.
#
# Never-delivered receives / never-drained sends mean a missed wake would trip the
# harness wall-clock timeout rather than pass.
import time

# --- A: three-deep nesting, innermost deadline ----------------------------------
def deep_child(
    channel[int64] ch,
    Atomic[int64] caught,
    Atomic[int64] cleaned,
    Atomic[int64] woke,
):
    try:
        scope:  # L1, no deadline
            scope:  # L2, no deadline
                scope(timeout=25):  # L3, innermost, the only deadline
                    try:
                        select:
                            case v = <-ch:
                                woke.add(1)  # unreachable
                    finally:
                        cleaned.add(1)
    except ScopeTimeout:
        caught.add(1)
    except Cancelled:
        woke.add(1000)  # inner deadline must surface as ScopeTimeout, not Cancelled

# --- B: multi-case select, deadline on the spawn scope itself --------------------
def multi_waiter(
    channel[int64] a,
    channel[int64] b,
    channel[int64] c,
    Atomic[int64] cleaned,
    Atomic[int64] woke,
):
    try:
        select:
            case v = <-a:
                woke.add(1)  # unreachable: nothing is ever sent on a
            case w = <-b:
                woke.add(1)  # unreachable: nothing is ever sent on b
            case c <- 7:
                woke.add(1)  # unreachable: nobody ever receives from c
    finally:
        cleaned.add(1)

# --- C: a sibling parked on a select, cancelled by a sibling failure -------------
# The `entered` rendezvous makes `boom` raise only after `sib_select` has passed
# the barrier and is entering the `select`, so the cancel lands on the PARKED
# select (the actual bug path — `scope_request_cancel` waking it) rather than the
# pre-park guard, deterministically across iterations.
def boom(channel[int64] entered):
    auto ready = <-entered
    raise ValueError("sibling boom")

def sib_select(
    channel[int64] entered,
    channel[int64] ch,
    Atomic[int64] cancelled,
    Atomic[int64] cleaned,
):
    try:
        entered <- 1
        select:
            case v = <-ch:
                cancelled.add(1000)  # unreachable: nothing is ever sent
    except Cancelled:
        cancelled.add(1)
    finally:
        cleaned.add(1)

def main():
    # A
    Atomic[int64] a_caught = Atomic(0)
    Atomic[int64] a_cleaned = Atomic(0)
    Atomic[int64] a_woke = Atomic(0)
    auto i = 0
    while i < 4:
        auto ch = channel[int64]()
        scope(timeout=3000, grace=500):
            spawn deep_child(ch, a_caught, a_cleaned, a_woke)
        if a_woke.load() != 0:
            raise ValueError("A: innermost deadline did not cancel the select cleanly")
        if a_caught.load() != i + 1 or a_cleaned.load() != i + 1:
            raise ValueError("A: three-deep nested deadline did not wake+drain once")
        i = i + 1
    print("A: innermost of three nested scopes woke the select")

    # B
    Atomic[int64] b_cleaned = Atomic(0)
    Atomic[int64] b_woke = Atomic(0)
    Atomic[int64] b_timeout = Atomic(0)
    i = 0
    while i < 4:
        auto a = channel[int64]()
        auto b = channel[int64]()
        auto c = channel[int64]()
        try:
            scope(timeout=25, grace=3000):
                spawn multi_waiter(a, b, c, b_cleaned, b_woke)
        except ScopeTimeout:
            b_timeout.add(1)
        if b_woke.load() != 0:
            raise ValueError("B: a multi-case select case fired with nothing ready")
        if b_cleaned.load() != i + 1:
            raise ValueError("B: multi-case select cleanup did not run exactly once")
        if b_timeout.load() != i + 1:
            raise ValueError("B: the scope deadline did not wake the multi-case select")
        i = i + 1
    print("B: multi-case select woken exactly once by the deadline")

    # C
    Atomic[int64] c_cancelled = Atomic(0)
    Atomic[int64] c_cleaned = Atomic(0)
    i = 0
    while i < 4:
        auto entered = channel[int64]()
        auto ch = channel[int64]()
        try:
            scope:
                spawn sib_select(entered, ch, c_cancelled, c_cleaned)
                spawn boom(entered)
        except ValueError:
            pass
        if c_cancelled.load() != i + 1:
            raise ValueError(
                "C: parked select was not cancelled by the sibling failure",
            )
        if c_cleaned.load() != i + 1:
            raise ValueError("C: parked select cleanup did not run exactly once")
        i = i + 1
    print("C: sibling failure cancelled and drained the parked select")
    print("variants: 3")
