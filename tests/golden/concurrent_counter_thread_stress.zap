# concurrent.Counter under real OS-thread parallelism (ticket #184, D-0078).
# Mirrors concurrent_counter_coro_stress but over `threading.Thread` (I#175): N
# OS threads each do `iters` atomic `add(1)` on one shared lock-free Counter,
# joined by the caller. Each Thread is a coroutine pinned to its own dedicated OS
# thread (beyond MAXPROCS), so the increments contend across cores and the shared
# cell is still reached through ordinary shadow-stack roots. The exact total
# (8 * 10000) proves no lost updates across both collectors. The lambda captures
# only the loop-invariant `c`/`iters`. No heap-byte assertions.
import concurrent

def worker(Counter c, int64 iters):
    for i in range(0, iters):
        c.add(1)

def main():
    Counter c = concurrent.Counter()
    auto iters = 10000
    list[Thread] ts = []
    for k in range(0, 8):
        ts.append(threading.Thread(target=lambda: worker(c, iters)))
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    print(c.get())
