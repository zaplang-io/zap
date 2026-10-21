# Golden (I#175): N Python-threading-shaped OS threads each do atomic add(1) on
# ONE shared Atomic[int64] box, joined by the caller. Mirrors atomic_counter but
# over real OS threads instead of coroutines: each Thread is a coroutine pinned
# to its own dedicated OS thread (beyond MAXPROCS), so the shared box is reached
# through the ordinary shadow-stack roots and the add is still an indivisible
# atomicrmw. The exact total (8 * 10000) proves no lost updates and a clean join
# across both collectors; no heap-byte assertions (order-independent sum).
def worker(Atomic[int64] a, int64 iters):
    for i in range(0, iters):
        a.add(1)

def main() -> int64:
    Atomic[int64] a = Atomic(0)
    auto iters = 10000
    list[Thread] ts = []
    for k in range(0, 8):
        ts.append(threading.Thread(target=lambda: worker(a, iters)))
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    print(a.load())
    return 0
