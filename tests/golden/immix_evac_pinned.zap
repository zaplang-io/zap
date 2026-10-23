# GC evacuation + pinning of a parked coroutine's roots (ticket #120). A worker
# coroutine builds many heap strings but keeps only a sparse subset (every 100th),
# signals it is ready, then PARKS on a channel receive. While it is parked, the
# main coroutine churns garbage and forces several collections: the worker's dead
# strings are reclaimed, leaving its survivors scattered in now-sparse blocks that
# the collector evacuates — relocating their string DATA buffers. The worker's
# roots live in its own green stack (a scheduler-owned, PINNED buffer the collector
# must never move) and its saved shadow chain; the moving collector walks that
# chain and rewrites each relocated reference in place. When the worker resumes it
# prints the survivors, reproducing their exact bytes — proving the pinned
# coroutine stack stayed put while the objects it references were moved and every
# reference was updated. A mis-pinned stack or an un-updated root would print
# garbage or crash. Deterministic stdout; identical under the moving Immix
# collector and non-moving Boehm.
def itos(int64 n) -> str:
    if n == 0:
        return "0"
    list[str] digits = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"]
    auto s = ""
    auto x = n
    while x > 0:
        s = digits[x % 10] + s
        x = x // 10
    return s

def worker(channel[int64] ready, channel[int64] go, channel[int64] done):
    list[str] keep = []
    for i in range(0, 4000):
        auto s = "payload-" + itos(i)
        if i % 100 == 0:
            keep.append(s)
    ready <- 1  # built; now park on `go` (roots held in the pinned stack)
    auto g0 = <-go
    for s in keep:
        print(s)
    done <- 1

def churn(int64 n):
    for j in range(0, n):
        auto g = "trash-" + itos(j)

def main():
    auto ready = channel[int64]()
    auto go = channel[int64]()
    auto done = channel[int64]()
    spawn worker(ready, go, done)
    auto r1 = <-ready  # the worker has built its list and is parked on `go`
    for r in range(0, 8):
        churn(8000)
        gc.collect()  # evacuate the parked worker's string buffers
    go <- 1
    auto r2 = <-done
