# Golden fixture: Once across many contending goroutines (ticket #103, D-0032).
# Eight workers race to run the initializer; `once_begin` lets exactly one
# through (the winner records a single tick on the shared deque and calls
# once_end), while the losers that arrive mid-init park until once_end and
# every later caller returns false at once. The recorded tick count is therefore
# always exactly 1, however the scheduler interleaves the workers — so the
# output is deterministic. A WaitGroup joins the workers before the count is
# read.
def worker(bytes o, bytes d, bytes wg):
    if sync.once_begin(o):
        deque.append(d, 1)
        sync.once_end(o)
    sync.done(wg)

def main():
    auto o = sync.once()
    auto d = deque.new()
    auto wg = sync.waitgroup()
    auto n = 8
    auto k = 0
    while k < n:
        sync.add(wg, 1)
        spawn worker(o, d, wg)
        k = k + 1
    sync.wait(wg)
    print(deque.len(d))
