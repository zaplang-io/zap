# Golden fixture (#1398): the walled garden around the nested-scope deadline fix.
# Three shapes the one-level `scope_nested_deadline_wakes_sleep` fixture does not
# cover, each asserted over several iterations:
#
#   A. THREE-DEEP lexical nesting. Only the INNERMOST scope carries the 25 ms
#      deadline; two enclosing deadline-less scopes wrap it. The sleep must bind
#      to the innermost scope, wake on its deadline, surface `ScopeTimeout`, and
#      let that `ScopeTimeout` propagate out through both enclosing scopes
#      unchanged (they neither swallow nor relabel it).
#
#   B. NO extra nesting: the sleeper is spawned straight into a timed scope and
#      sleeps in that scope's body with no inner `scope`. This is the pre-#1398
#      deadline path (the sleep's innermost scope IS its spawn scope) and MUST
#      stay working — the fix must not regress the already-correct case.
#
#   C. A spawned SIBLING parked on a channel receive (not a sleep) between
#      "collections" — the shape the issue notes already works — must still be
#      woken by the scope deadline and observe `Cancelled`. Guards the matching
#      channel-waiter binding against the same regression.
#
# Hour-long sleeps / never-delivered receives mean a missed wake would trip the
# harness wall-clock timeout rather than pass.
import time

# --- A: three-deep nesting, innermost deadline ----------------------------------
def deep_child(Atomic[int64] caught, Atomic[int64] cleaned, Atomic[int64] woke):
    try:
        scope:  # L1, no deadline
            scope:  # L2, no deadline
                scope(timeout=25):  # L3, innermost, the only deadline
                    try:
                        time.sleep_for(time.seconds(3600))
                        woke.add(1)  # unreachable
                    finally:
                        cleaned.add(1)
    except ScopeTimeout:
        caught.add(1)
    except Cancelled:
        woke.add(1000)  # inner deadline must surface as ScopeTimeout, not Cancelled

# --- B: no extra nesting, deadline on the spawn scope itself ---------------------
def flat_sleeper(Atomic[int64] cleaned, Atomic[int64] woke):
    try:
        time.sleep_for(time.seconds(3600))
        woke.add(1)  # unreachable: the spawn scope's deadline cancels this
    finally:
        cleaned.add(1)

# --- C: a sibling parked on a channel receive ------------------------------------
def waiting_reader(channel[int64] ch, Atomic[int64] cancelled, Atomic[int64] delivered):
    try:
        auto v = <-ch
        delivered.add(1)  # unreachable: nothing is ever sent
    except Cancelled:
        cancelled.add(1)

def main():
    # A
    Atomic[int64] a_caught = Atomic(0)
    Atomic[int64] a_cleaned = Atomic(0)
    Atomic[int64] a_woke = Atomic(0)
    auto i = 0
    while i < 4:
        scope(timeout=3000, grace=500):
            spawn deep_child(a_caught, a_cleaned, a_woke)
        if a_woke.load() != 0:
            raise ValueError("A: innermost deadline did not cancel the sleep cleanly")
        if a_caught.load() != i + 1 or a_cleaned.load() != i + 1:
            raise ValueError("A: three-deep nested deadline did not wake+drain once")
        i = i + 1
    print("A: innermost of three nested scopes woke the sleep")

    # B
    Atomic[int64] b_cleaned = Atomic(0)
    Atomic[int64] b_woke = Atomic(0)
    Atomic[int64] b_timeout = Atomic(0)
    i = 0
    while i < 4:
        try:
            scope(timeout=25, grace=3000):
                spawn flat_sleeper(b_cleaned, b_woke)
        except ScopeTimeout:
            b_timeout.add(1)
        if b_woke.load() != 0:
            raise ValueError("B: flat deadline failed to cancel the sleep")
        if b_cleaned.load() != i + 1 or b_timeout.load() != i + 1:
            raise ValueError("B: flat (non-nested) deadline path regressed")
        i = i + 1
    print("B: flat scope deadline still wakes the sleep")

    # C
    Atomic[int64] c_cancelled = Atomic(0)
    Atomic[int64] c_delivered = Atomic(0)
    Atomic[int64] c_timeout = Atomic(0)
    i = 0
    while i < 4:
        auto ch = channel[int64]()
        try:
            scope(timeout=25, grace=3000):
                spawn waiting_reader(ch, c_cancelled, c_delivered)
        except ScopeTimeout:
            c_timeout.add(1)
        if c_delivered.load() != 0:
            raise ValueError("C: reader received a value that was never sent")
        if c_cancelled.load() != i + 1 or c_timeout.load() != i + 1:
            raise ValueError(
                "C: channel-parked sibling was not cancelled by the deadline",
            )
        i = i + 1
    print("C: channel-parked sibling still cancelled by the deadline")
    print("variants: 3")
