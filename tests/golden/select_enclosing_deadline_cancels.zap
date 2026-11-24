# Golden fixture (#1414, D-0094 the other way): when the ENCLOSING scope's
# deadline (not the inner scope's own) cancels a `select` parked in an inner
# lexical `scope` that has NO deadline of its own, the `select` must still wake at
# once, but the inner scope must propagate the cancellation as a raw `Cancelled`
# (it did not time out) — only the OUTER scope, whose deadline fired, re-labels it
# `ScopeTimeout` to its caller. This is the complement of
# `select_scope_deadline_wakes` and pins both halves of the D-0094 rule for a
# parked `select`:
#   * a scope's OWN deadline  -> ScopeTimeout   (that fixture)
#   * an enclosing deadline   -> Cancelled      (this fixture)
# The #1414 fix binds the select's per-case waiters to the innermost lexical
# scope; this fixture proves that binding is still a DESCENDANT of every enclosing
# scope, so an outer deadline reaches it, and that the inner scope does not
# spuriously claim the timeout. `grace` is wide so the woken child drains
# cooperatively (its `finally` runs) rather than being severed. The channel is
# never sent to, so a missed wake blocks forever. Repeated for interleaving
# robustness.
import time

def child(
    channel[int64] ch,
    Atomic[int64] cancelled,
    Atomic[int64] mislabelled,
    Atomic[int64] cleaned,
):
    try:
        scope:  # inner lexical scope, NO deadline of its own
            try:
                select:
                    case v = <-ch:
                        mislabelled.add(1000)  # unreachable: nothing is sent
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
        auto ch = channel[int64]()
        try:
            scope(timeout=25, grace=3000):
                spawn child(ch, cancelled, mislabelled, cleaned)
        except ScopeTimeout:
            outer_timeout.add(1)
        if cancelled.load() != i + 1:
            raise ValueError("inner select did not observe the enclosing cancellation")
        if mislabelled.load() != 0:
            raise ValueError("enclosing cancel surfaced with the wrong label")
        if cleaned.load() != i + 1:
            raise ValueError("inner body cleanup did not run exactly once")
        if outer_timeout.load() != i + 1:
            raise ValueError("the enclosing scope did not raise ScopeTimeout")
        i = i + 1
    print("enclosing deadline cancelled the parked select as Cancelled")
    print("iterations: 8")
