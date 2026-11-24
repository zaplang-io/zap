# Golden fixture (#1398, D-0094 the other way): when the ENCLOSING scope's
# deadline (not the inner scope's own) cancels a `time.sleep_for` sleeping in an
# inner lexical `scope` that has NO deadline of its own, the wait must still wake
# at once, but the inner scope must propagate the cancellation as a raw
# `Cancelled` (it did not time out) — only the OUTER scope, whose deadline fired,
# re-labels it `ScopeTimeout` to its caller. This is the complement of
# `scope_nested_deadline_wakes_sleep` and pins both halves of the D-0094 rule:
#   * a scope's OWN deadline  -> ScopeTimeout   (that fixture)
#   * an enclosing deadline   -> Cancelled      (this fixture)
# The fix for #1398 binds the sleep to the innermost lexical scope; this fixture
# proves that binding is still a DESCENDANT of every enclosing scope, so an outer
# deadline reaches it, and that the inner scope does not spuriously claim the
# timeout. `grace` is wide so the woken child drains cooperatively (its `finally`
# runs) rather than being severed. Repeated for interleaving robustness.
import time

def child(Atomic[int64] cancelled, Atomic[int64] mislabelled, Atomic[int64] cleaned):
    try:
        scope:  # inner lexical scope, NO deadline of its own
            try:
                time.sleep_for(time.seconds(3600))
                mislabelled.add(1000)  # unreachable: the outer deadline cancels us
            finally:
                cleaned.add(1)
    except ScopeTimeout:
        # Would mean the inner scope wrongly claimed the enclosing scope's timeout.
        mislabelled.add(1)
    except Cancelled:
        cancelled.add(1)

def main():
    Atomic[int64] cancelled = Atomic(0)
    Atomic[int64] mislabelled = Atomic(0)
    Atomic[int64] cleaned = Atomic(0)
    Atomic[int64] outer_timeout = Atomic(0)
    auto i = 0
    while i < 8:
        try:
            scope(timeout=25, grace=3000):
                spawn child(cancelled, mislabelled, cleaned)
        except ScopeTimeout:
            outer_timeout.add(1)
        if cancelled.load() != i + 1:
            raise ValueError("inner body did not observe the enclosing cancellation")
        if mislabelled.load() != 0:
            raise ValueError("enclosing cancel surfaced with the wrong label")
        if cleaned.load() != i + 1:
            raise ValueError("inner body cleanup did not run exactly once")
        if outer_timeout.load() != i + 1:
            raise ValueError("the enclosing scope did not raise ScopeTimeout")
        i = i + 1
    print("enclosing deadline cancelled the nested sleep as Cancelled")
    print("iterations: 8")
