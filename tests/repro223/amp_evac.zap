def worker(channel[int64] ch, int64 id, int64 reps):
    list[int64] keep = []
    for i in range(0, reps):
        auto garbage = [i, i + 1, i + 2]
        auto g2 = [i, i, i, i, i, i, i, i]
        if i % 10 == 0:
            keep.append(i + id)
        if i % 1000 == 0:
            gc.collect()
    auto acc = 0
    auto n = 0
    for v in keep:
        acc = acc + v
        n = n + 1
        if n % 100 == 0:
            gc.collect()
    ch <- acc

def main():
    auto ch = channel[int64]()
    spawn worker(ch, 0, 20000)
    spawn worker(ch, 1, 20000)
    spawn worker(ch, 2, 20000)
    spawn worker(ch, 3, 20000)
    spawn worker(ch, 4, 20000)
    spawn worker(ch, 5, 20000)
    spawn worker(ch, 6, 20000)
    spawn worker(ch, 7, 20000)
    auto total = 0
    auto k = 0
    while k < 8:
        total = total + <-ch
        k = k + 1
    print(total)
