# Golden fixture: goroutines + channels (ticket #25, D-0013/D-0031).
#
# Spawns several coroutines, each of which sends one value over a shared
# unbuffered channel; `main` receives them and sums. The sum is
# order-independent, so the output is deterministic regardless of how the
# scheduler interleaves the goroutines — this proves real concurrency and
# channel blocking (each unbuffered send rendezvouses with a receive) without a
# race on the printed result.

def worker(channel[int64] ch, int64 v):
    ch <- v

def main():
    auto ch = channel[int64]()
    spawn worker(ch, 10)
    spawn worker(ch, 20)
    spawn worker(ch, 30)
    spawn worker(ch, 40)
    auto total = 0
    auto i = 0
    while i < 4:
        auto x = <-ch
        total = total + x
        i = i + 1
    print(total)

    # A buffered channel exercises the non-blocking buffered path and `close`:
    # fill it, close it, then drain. Draining a producer/consumer total is also
    # order-independent.
    auto buf = channel[int64](3)
    buf <- 1
    buf <- 2
    buf <- 3
    close(buf)
    auto sum = 0
    auto j = 0
    while j < 3:
        sum = sum + <-buf
        j = j + 1
    print(sum)
