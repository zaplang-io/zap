# Golden (I#185): `with lock:` is the idiomatic mutex guard. N OS threads each
# increment ONE shared non-atomic cell guarded by `with lk:` (the context-manager
# form of threading.Lock, lowered via __enter__/__exit__). The exact total
# (8 * 5000) proves the guard acquires on entry AND releases on normal loop-body
# exit every iteration — a dropped release would deadlock (harness timeout) and a
# missing acquire would lose updates. Mirrors threading_lock.zap but through the
# `with` protocol instead of explicit acquire()/release(). No heap-byte asserts.
def worker(list[int64] cell, Lock lk, int64 iters):
    for i in range(0, iters):
        list[int64] junk = [i, i, i]
        with lk:
            cell[0] = cell[0] + 1

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
