# Golden fixture: M:N work-stealing scheduler stress (ticket #47, D-0008).
#
# Spawns far more goroutines than there are worker threads (M), so the program
# only produces the right answer if the Gs really multiplex over the Ms via the
# user-space scheduler (parking a blocked G and freeing its M). Both totals are
# order-independent, so the output is deterministic no matter how the scheduler
# interleaves — run with `ZAP_MAXPROCS=1` to force all Gs onto a single M.

def sender(channel[int64] ch, int64 v):
    ch <- v

def main():
    # 200 goroutines each send their index over an unbuffered channel. Every
    # send rendezvouses with a receive in `main`; the sum 0+1+...+199 = 19900 is
    # independent of the order the sends are serviced. With fewer Ms than Gs,
    # most senders park on the channel and are resumed as `main` receives.
    auto ch = channel[int64]()
    auto n = 0
    while n < 200:
        spawn sender(ch, n)
        n = n + 1
    auto total = 0
    auto got = 0
    while got < 200:
        total = total + <-ch
        got = got + 1
    print(total)

    # A small buffered channel with many more senders than capacity: senders
    # block (park) when the buffer is full and are woken as `main` drains it.
    # 50 sends of 2 sum to 100 regardless of interleaving.
    auto bch = channel[int64](8)
    auto s = 0
    while s < 50:
        spawn sender(bch, 2)
        s = s + 1
    auto sum = 0
    auto k = 0
    while k < 50:
        sum = sum + <-bch
        k = k + 1
    print(sum)
