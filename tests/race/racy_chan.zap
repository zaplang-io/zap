# Walled-garden fixture for the runtime data-race detector (`--race`, #665,
# D-0114; slice 3 breadth, #679). A SOUNDNESS guard for the channel edge: a
# channel in the program must NOT fabricate a happens-before edge where none
# exists. Two `worker` coroutines each write the plain, non-`@shareable` mutable
# MODULE GLOBAL `counter` with NO synchronization between them, then each sends a
# token to `main` on a SHARED channel.
#
# The channel's send/recv edges order each worker before `main`'s receive, but do
# NOT order the two workers against each other (each sends its own token; neither
# receives the other's). So the two writes to `counter` still race. `--race` MUST
# report the data race on `counter` and exit non-zero — if it did not, the
# channel edge would be over-synchronizing (a false negative: a hidden race),
# which is the failure this fixture guards against. The verdict is
# schedule-independent (there is no ordering edge between the writers).
#
# Expected (detector slice):
#   $ zap run --race tests/race/racy_chan.zap
#   -> a data-race report for `counter` with a source location; non-zero exit.
int64 counter = 0


def worker(channel[int64] ch, int64 v):
    counter = v
    ch <- v


def main():
    auto ch = channel[int64]()
    spawn worker(ch, 1)
    spawn worker(ch, 2)
    auto a = <-ch
    auto b = <-ch
    print(counter)
