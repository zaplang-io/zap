# GC stress for captured closure environments (I#163). Four coroutines each
# allocate many closures that capture a *pointer* (a freshly built `str`), then
# call each closure after further allocations have forced collections mid-loop.
# The captured `str` is reachable ONLY through the closure record's environment,
# so correct output under BOTH collectors proves the precise collector scans the
# environment's pointer captures via its type descriptor: a wrong map would drop
# the `str` (use-after-free / garbage length) or trace a non-pointer word.
# The per-iteration work (len("item") == 4) makes the sum order-independent, so
# the result is deterministic regardless of scheduler interleaving.
def make_tagger(str tag) -> fn(int64) -> str:
    return lambda x: tag

def worker(channel[int64] ch, int64 reps):
    auto acc = 0
    for i in range(0, reps):
        auto f = make_tagger("item")
        auto s = f(i)
        acc = acc + len(s)
    ch <- acc

def main():
    auto ch = channel[int64]()
    spawn worker(ch, 20000)
    spawn worker(ch, 20000)
    spawn worker(ch, 20000)
    spawn worker(ch, 20000)
    auto total = 0
    auto k = 0
    while k < 4:
        total = total + <-ch
        k = k + 1
    print(total)
