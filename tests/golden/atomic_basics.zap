# Golden: single-threaded coverage of every Atomic[T] operation (I#176). Proves
# the load/store/add/sub/swap/compare_and_swap lowering to LLVM atomics is
# correct in isolation (concurrency is exercised by atomic_counter, atomic_cas,
# and atomic_bool). `add`/`sub` return the NEW value (Go's `atomic.Int64`
# semantics); `swap` and a failed `compare_and_swap` return the prior value /
# `False`. `int32` rides the same path at a narrower atomic width; `bool` is
# stored as a byte-wide atomic (an `i1` cell accessed as `i8`). Boolean results
# are printed as 1/0 via `b2i` (bare `print(bool)` is unrelated Zap formatting).
def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def main():
    Atomic[int64] a = Atomic(10)
    print(a.load())
    a.store(100)
    print(a.load())
    print(a.add(5))
    print(a.sub(30))
    print(a.load())
    print(a.swap(1000))
    print(a.load())
    print(b2i(a.compare_and_swap(1000, 7)))
    print(a.load())
    print(b2i(a.compare_and_swap(1000, 9)))
    print(a.load())

    Atomic[int32] b = Atomic(1)
    print(b.add(2))
    print(b.load())

    Atomic[bool] flag = Atomic(False)
    print(b2i(flag.load()))
    flag.store(True)
    print(b2i(flag.load()))
    print(b2i(flag.swap(False)))
    print(b2i(flag.compare_and_swap(False, True)))
    print(b2i(flag.load()))
