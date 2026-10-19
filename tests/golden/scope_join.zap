# Golden fixture: structured concurrency — a `scope:` joins its coroutines
# (ticket #50, D-0044, design/0005-concurrency.md "Structured concurrency").
#
# The scope owns the two spawned workers and does not let control past the
# dedent until both have finished (the full join barrier). Each worker deposits
# one value into a buffered channel; `main` drains the channel *after* the scope
# and sums — which is only safe because the join guarantees both workers ran.
# The sum is order-independent, so the result is deterministic.

def worker(channel[int64] ch, int64 v):
    ch <- v

def main():
    auto ch = channel[int64](2)
    scope:
        spawn worker(ch, 10)
        spawn worker(ch, 20)
    # Dedent: both workers have completed. Drain their results.
    auto total = 0
    auto i = 0
    while i < 2:
        total = total + <-ch
        i = i + 1
    print(total)
