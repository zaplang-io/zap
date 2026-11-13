# Golden fixture: coroutine context switches racing a stop-the-world collection
# (I#885, gc-boehm).
#
# Under the Boehm collector a stopped thread's stack is scanned as
# `[sp, stack_end)`. The scheduler used to move `stack_end` to the coroutine's
# stack top BEFORE the context switch that moves `sp` there (and back to the
# native stack AFTER switching back), so a stop-the-world landing in either gap
# saw `sp` on one stack and `stack_end` on the other: Boehm scanned a multi-MB
# range spanning unrelated mappings, and a mark thread SIGSEGV-ed in an unmapped
# hole. A program that switches constantly sits in that window a large fraction of
# the time, so it died ~80% of runs.
#
# Every coroutine below yields through the scheduler on EVERY loop iteration
# (`checkpoint()` is a cooperative yield = a full switch out and back in) while
# allocating, so collections are frequent and, with 16 coroutines on the
# multi-worker scheduler, almost always catch some worker mid-switch. The totals
# are order-independent, so the output is deterministic however the scheduler
# interleaves them.

def spinner(int64 n, channel[int64] done):
    list[int64] keep = [0]
    int64 i = 0
    while i < n:
        checkpoint()
        if i % 16 == 0:
            keep = [i, i + 1, i + 2, i + 3]
        i = i + 1
    done <- n + keep[0]

def main():
    auto done = channel[int64](64)
    for k in range(16):
        spawn spinner(3000, done)
    int64 total = 0
    for k in range(16):
        total = total + <-done
    print("total", total)
