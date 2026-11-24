# GC (ticket #48): collection across a parked-coroutine boundary — the precise
# replacement for Boehm's "suspended stack is traced as heap" trick. Two workers
# each build a heap string and block sending it on an unbuffered channel; while
# they are parked, `main` allocates enough garbage to trigger collections. The
# parked coroutines' live strings must survive (reached via each coroutine's
# saved shadow chain), so the received values print correctly.
def worker(channel[str] ch, str msg):
    ch <- msg

def main():
    auto ch = channel[str]()
    spawn worker(ch, "alpha")
    spawn worker(ch, "beta")
    auto g = 0
    for i in range(0, 100000):
        auto junk = [i, i, i]
        g = g + junk[0]
    auto a = <-ch
    auto b = <-ch
    print(g)
    # The two workers race to send on the unbuffered channel, so the receive
    # order is scheduler-dependent (and perturbed by the wide CI e2e pool,
    # zaplang-io/zap#1305). The GC claim under test is "both parked coroutines'
    # live strings survive collection", which is order-independent — so sort the
    # received pair before printing to keep this golden deterministic.
    for s in sorted([a, b]):
        print(s)
