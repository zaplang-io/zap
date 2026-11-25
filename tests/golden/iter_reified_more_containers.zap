# Golden fixture: reified iterators across the remaining container kinds — heap /
# deque / oset / tuple[T] (#1382 phase 4, completing the per-container coverage of
# phase 3's list/set/dict/str/bytes/range). `iter(c)` returns a precise reified
# iterator that yields the same elements, in the same order, as `for x in c`:
# heap → priority (min-first) order, deque → left-to-right, oset → insertion
# order, tuple[T] → construction order. `next(iter(c))` is the exact element type.

def main():
    # --- heap[int64]: priority (min-first) order, non-draining ---
    auto h = heap[int64]()
    h.push(5)
    h.push(1)
    h.push(3)
    print(next(iter(h)))  # 1   (smallest first)
    int64 hsum = 0
    for x in iter(h):
        hsum = hsum + x
    print(hsum)  # 9   (1 + 3 + 5; iteration does not drain)
    assert hsum == 9
    assert len(h) == 3

    # --- deque[int64]: left-to-right ---
    auto dq = deque[int64]()
    dq.append(10)
    dq.append(20)
    dq.append(30)
    auto di = iter(dq)
    print(next(di))  # 10
    print(next(di))  # 20
    print(next(di))  # 30
    print(next(di, -1))  # -1  (exhausted -> default)
    assert next(di, -1) == -1

    # --- oset[int64]: insertion order, duplicates ignored ---
    auto os = oset[int64]()
    os.add(7)
    os.add(3)
    os.add(7)
    os.add(5)
    auto oi = iter(os)
    print(next(oi))  # 7
    print(next(oi))  # 3
    print(next(oi))  # 5

    # --- tuple[int64]: construction order ---
    auto t = tuple([100, 200])
    auto ti = iter(t)
    print(next(ti))  # 100
    print(next(ti))  # 200
    int64 tfirst = next(iter(tuple([42, 43])))
    print(tfirst)  # 42
    assert tfirst == 42

    # --- StopIteration past the end of a reified container iterator ---
    auto e = iter(deque[int64]())
    bool raised = False
    try:
        auto dead = next(e)
        print(dead)  # (never reached)
    except StopIteration:
        raised = True
    print(raised)  # True
    assert raised
