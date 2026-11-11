# Surface coverage for the built-in AtomicArray[T] (#844): fixed-capacity array
# of seq_cst CAS-able slots. Covers len(), zero-initialised load, store, and
# compare_and_swap (success, then a stale failure that leaves the slot intact),
# for both a word element (int64) and a reference element (str). CAS is
# POINTER-IDENTITY (like every concurrent.* structure, D-0078), so the reference
# case reuses the same `str` object for the expected value. Deterministic;
# identical under both collectors; no heap-byte assertions.

def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def main():
    # Word slots (int64).
    AtomicArray[int64] a = AtomicArray[int64](4)
    print(a.len())  # 4
    print(a.load(0))  # 0  (zero-initialised)
    a.store(2, 42)
    print(a.load(2))  # 42
    print(b2i(a.compare_and_swap(2, 42, 99)))  # 1  (matches -> swapped)
    print(a.load(2))  # 99
    print(b2i(a.compare_and_swap(2, 42, 7)))  # 0  (stale expected -> no swap)
    print(a.load(2))  # 99 (unchanged)

    # Reference slots (str): CAS compares object identity, so reuse `a0`.
    auto a0 = "alpha"
    auto b0 = "beta"
    AtomicArray[str] s = AtomicArray[str](2)
    s.store(0, a0)
    print(s.load(0))  # alpha
    print(b2i(s.compare_and_swap(0, a0, b0)))  # 1  (identity match -> swapped)
    print(s.load(0))  # beta
    print(b2i(s.compare_and_swap(0, a0, a0)))  # 0  (slot now b0 -> no swap)
    print(s.load(0))  # beta
