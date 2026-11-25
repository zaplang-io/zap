# Golden fixture: reified per-container iterators — iter() / next() over a
# built-in container (#1382 phase 3). `iter(c)` now returns a precise reified
# iterator (a typed synthesized generator) instead of the phase-2 error, carrying
# the container's exact element type, so `next(iter(xs))` is the exact `T` and
# `for` / `next` / `next(it, default)` all work. The element type matches `for`'s:
# list → T, str → char, bytes/range → int64, dict → key, set → element. Order is
# deterministic only for sequences; the dict/set cases below use a single element.

def main():
    # --- list: hand-stepping + default at exhaustion ---
    auto li = iter([10, 20, 30])
    print(next(li))  # 10
    print(next(li))  # 20
    print(next(li, -1))  # 30
    print(next(li, -1))  # -1  (exhausted -> default)
    assert next(li, -1) == -1

    # --- for over iter(list), and the exact element type feeds arithmetic ---
    int64 total = 0
    for x in iter([1, 2, 3, 4]):
        total = total + x
    print(total)  # 10
    int64 first = next(iter([7, 8, 9]))
    print(first + 1)  # 8
    assert first == 7

    # --- str -> char (via ord for an unambiguous int) ---
    auto si = iter("ab")
    print(ord(next(si)))  # 97
    print(ord(next(si)))  # 98

    # --- bytes -> int64 (raw byte values) ---
    auto bi = iter(b"AB")
    print(next(bi))  # 65
    print(next(bi))  # 66

    # --- range -> int64 ---
    auto ri = iter(range(3))
    print(next(ri))  # 0
    print(next(ri))  # 1
    print(next(ri))  # 2

    # --- next(it) past the end raises a catchable StopIteration ---
    auto e = iter([42])
    print(next(e))  # 42
    bool raised = False
    try:
        auto dead = next(e)
        print(dead)  # (never reached)
    except StopIteration:
        raised = True
    print(raised)  # True
    assert raised

    # --- dict iterates keys; single entry keeps it deterministic ---
    dict[str, int64] d = {"only": 5}
    print(next(iter(d)))  # only

    # --- set element; single element keeps it deterministic ---
    set[int64] s = set([7])
    print(next(iter(s)))  # 7
