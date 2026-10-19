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
    print(a)
    print(b)
