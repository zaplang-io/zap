# Golden fixture: channel[float64] (#800 Phase 1).
#
# A channel carries float elements as reinterpreted words. Mirrors coroutines.zap:
# several spawned coroutines each send one float over a shared *unbuffered*
# channel and `main` sums them (each send rendezvouses with a receive, proving the
# blocking path), then a *buffered* channel is filled, closed, and drained. Both
# totals are order-independent, so the output is deterministic regardless of
# scheduling. Values format exactly (halves/quarters).
def worker(channel[float64] ch, float64 v):
    ch <- v

def main():
    auto ch = channel[float64]()
    spawn worker(ch, 10.5)
    spawn worker(ch, 20.25)
    spawn worker(ch, 30.0)
    spawn worker(ch, 40.25)
    float total = 0.0
    auto i = 0
    while i < 4:
        float x = <-ch
        total = total + x
        i = i + 1
    print(total)   # 10.5 + 20.25 + 30.0 + 40.25 = 101.0

    # A buffered channel: fill, close, then drain the producer total.
    auto buf = channel[float64](3)
    buf <- 1.5
    buf <- 2.5
    buf <- 3.25
    close(buf)
    float sum = 0.0
    auto j = 0
    while j < 3:
        sum = sum + <-buf
        j = j + 1
    print(sum)     # 1.5 + 2.5 + 3.25 = 7.25
