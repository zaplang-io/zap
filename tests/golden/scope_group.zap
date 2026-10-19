# Golden fixture: structured concurrency — several owned coroutines failing
# concurrently surface as an aggregate `ExceptionGroup` (ticket #50, D-0045,
# design/0005-concurrency.md "Structured concurrency" rule 3).
#
# Both workers raise immediately (they reach no cancellation point, so neither
# is pre-empted by the other's failure): the scope collects *both* exceptions
# and re-raises them at the dedent as one `ExceptionGroup`, rather than dropping
# all but the first. The aggregate's sub-exceptions are rendered in a stable
# (sorted) order, so stderr is deterministic regardless of scheduling.
#
# The harness checks the exit code (scope_group.exit) and that stderr contains
# the aggregate message (scope_group.stderr); stdout is matched as usual.

def fail(str msg):
    raise msg

def main() -> int64:
    print("start")
    scope:
        spawn fail("alpha failed")
        spawn fail("beta failed")
    print("unreachable")
    return 0
