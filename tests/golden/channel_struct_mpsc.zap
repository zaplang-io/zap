# Golden fixture: multi-coroutine struct transfer over a channel (#1100).
#
# Mirrors the Savina / sort-result transfer in zap-benchmarks#21: several
# producer coroutines each deliver a struct over a shared *unbuffered* channel,
# and the single consumer reads every struct and sums a field. Each unbuffered
# send rendezvouses with a receive (proving the blocking handoff carries the
# struct type), and the sum is order-independent, so the output is deterministic
# regardless of scheduling. A second, buffered channel then fills/closes/drains
# to exercise the buffered struct path too.
struct Entry:
    int amount

def producer(channel[Entry] ch, int v):
    ch <- Entry(v)

def main():
    auto ch = channel[Entry]()
    spawn producer(ch, 10)
    spawn producer(ch, 20)
    spawn producer(ch, 30)
    spawn producer(ch, 40)
    auto total = 0
    auto i = 0
    while i < 4:
        auto e = <-ch
        total = total + e.amount
        i = i + 1
    print(total)  # 10 + 20 + 30 + 40 = 100

    # Buffered struct channel: fill, close, then drain and sum.
    auto buf = channel[Entry](3)
    buf <- Entry(1)
    buf <- Entry(2)
    buf <- Entry(3)
    close(buf)
    auto sum = 0
    auto j = 0
    while j < 3:
        auto d = <-buf
        sum = sum + d.amount
        j = j + 1
    print(sum)  # 1 + 2 + 3 = 6
