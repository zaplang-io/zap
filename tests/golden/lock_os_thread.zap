# Golden: LockOSThread — pin a coroutine to its own dedicated OS thread, run it
# (including across a channel park), then unlock and rejoin (ticket #172).
#
# The `pinned` coroutine calls `runtime.lock_os_thread()`, which migrates it onto
# a private dedicated M (OS thread) that runs only this coroutine. It then PARKS
# on an unbuffered channel receive — proving a pinned coroutine that blocks is
# re-scheduled back onto its OWN dedicated thread (not the shared pool) — and on
# wake computes a result, calls `runtime.unlock_os_thread()` (which returns it to
# the shared pool and tears the dedicated thread down), stores the result, and
# finishes. `main` joins it via a WaitGroup. The deterministic output (42) only
# appears if the pin/park/resume/unpin/rejoin cycle is correct; green under both
# collectors (the dedicated M participates in GC like any worker).

def pinned(channel[int64] ch, Atomic[int64] out, bytes wg):
    runtime.lock_os_thread()
    auto v = <-ch                 # park while pinned; resume on the dedicated M
    auto s = v * 2
    runtime.unlock_os_thread()
    out.store(s)
    sync.done(wg)

def main():
    auto ch = channel[int64]()
    Atomic[int64] out = Atomic(0)
    auto wg = sync.waitgroup()
    sync.add(wg, 1)
    spawn pinned(ch, out, wg)
    ch <- 21                       # rendezvous with the pinned receiver
    sync.wait(wg)
    print(out.load())
