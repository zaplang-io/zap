# Golden: an atomic bool flag claimed by exactly one of many racers (I#176).
# Eight coroutines each try `compare_and_swap(False, True)` on one shared
# `Atomic[bool]`; CAS guarantees exactly one sees the `False`->`True` transition,
# so exactly one increments the `Atomic[int64]` winner counter. The deterministic
# result (1 winner, flag left True) proves the byte-wide bool atomic and its CAS
# are race-free: two apparent winners would mean the CAS was not atomic. Joined by
# a WaitGroup; green under both collectors.
def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def worker(Atomic[bool] flag, Atomic[int64] winners, bytes wg):
    if flag.compare_and_swap(False, True):
        winners.add(1)
    sync.done(wg)

def main():
    Atomic[bool] flag = Atomic(False)
    Atomic[int64] winners = Atomic(0)
    auto wg = sync.waitgroup()
    for k in range(0, 8):
        sync.add(wg, 1)
        spawn worker(flag, winners, wg)
    sync.wait(wg)
    print(winners.load())
    print(b2i(flag.load()))
