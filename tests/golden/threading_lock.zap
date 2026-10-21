# Golden (I#175): N OS threads increment ONE shared, non-atomic cell guarded by
# a threading.Lock, each allocating per iteration to pressure the collector. The
# exact total (8 * 5000) proves the Lock serializes the read-modify-write (a
# lost update would lower it) AND that the moving Immix collector (#120)
# relocates safely while raw OS threads hold roots through the shadow stack. The
# cell is a one-element list (a shared heap box); deterministic, no heap-byte
# assertions.
def worker(list[int64] cell, Lock lk, int64 iters):
    for i in range(0, iters):
        list[int64] junk = [i, i, i]
        lk.acquire()
        cell[0] = cell[0] + 1
        lk.release()

def main() -> int64:
    list[int64] cell = [0]
    auto lk = threading.Lock()
    auto iters = 5000
    list[Thread] ts = []
    for k in range(0, 8):
        ts.append(threading.Thread(target=lambda: worker(cell, lk, iters)))
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    print(cell[0])
    return 0
