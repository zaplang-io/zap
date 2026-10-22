# Golden fixture: netpoller — non-blocking I/O readiness for the M:N scheduler
# (ticket #80, D-0008, design/0005-concurrency.md "Blocking syscalls").
#
# The netpoller has no language-level I/O surface yet (sockets are #79, signals
# #77), so the end-to-end exercise lives in the runtime behind the internal
# `__netpoll_selftest()` hook, which this golden invokes. The self-test runs the
# scheduler with a SINGLE worker M, then:
#
#   1. spawns a reader coroutine that blocks on a not-yet-ready pipe fd — it must
#      PARK its G and free the one M, or the writer coroutine (which wakes it)
#      could never be scheduled and the program would deadlock — and a writer
#      coroutine that makes the fd ready; the reader wakes on readiness and reads
#      the byte. This proves a blocked G parks and its M is freed to run others.
#   2. parks on a second fd that never becomes ready and confirms the wait
#      returns when its deadline expires.
#
# It returns the number of sub-checks that passed (2 on full success), so the
# output is deterministic no matter how the scheduler interleaves. The self-test
# runs identically under both collectors (Boehm default + precise Immix).

def main():
    print(__netpoll_selftest())
