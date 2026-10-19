# Golden fixture: structured concurrency — a failing coroutine cancels its
# siblings (ticket #50, D-0044, design/0005-concurrency.md "Structured
# concurrency" rule 3 + "Cancellation is cooperative").
#
# `spin` is a long-running sibling that cooperates with cancellation by calling
# `checkpoint()` each iteration. When `fail` raises, the scope (default
# OnError.Cancel) arms its children; `spin`'s next `checkpoint()` observes the
# cancel and raises `Cancelled`, which unwinds it without completing the loop —
# so "spin finished" is never printed. `Cancelled` is not itself a failure, so
# the scope re-raises only the single real failure ("worker failed") at the
# dedent. If cancellation did not work, `spin` would run to completion and print
# "spin finished", failing the stdout match.
#
# The harness checks the exit code (scope_cancel.exit) and that stderr carries
# the surviving failure (scope_cancel.stderr).

def fail():
    raise "worker failed"

def spin():
    auto i = 0
    while i < 100000000:
        checkpoint()
        i = i + 1
    print("spin finished")

def main() -> int64:
    print("start")
    scope:
        spawn fail()
        spawn spin()
    print("unreachable")
    return 0
