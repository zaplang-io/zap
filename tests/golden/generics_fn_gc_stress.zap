# GC stress for generic FUNCTIONS (I#161, building on the generic-type stress in
# generics_gc_stress). Four goroutines each churn many heap instances allocated
# THROUGH generic functions — make_pair[int64,str] (a pointer + scalar payload)
# and wrap[T] (Option[str] pointer payload, Option[int64] unboxed scalar) —
# forcing collections mid-loop. Correct output under BOTH collectors proves each
# monomorphized generic-fn instance produces instances with a correct
# per-instance ZapTypeDesc pointer map from codegen's existing per-struct
# emission (the "GC metadata is free" theorem holds for generic fns too): a wrong
# map would mis-trace the str pointer or follow the i64 as a pointer and crash.
# The summed result is order-independent, so deterministic regardless of
# scheduler interleaving.
struct Pair[A, B]:
    A first
    B second

def make_pair[A, B](A a, B b) -> Pair[A, B]:
    return Pair(a, b)

def wrap[T](T x) -> Option[T]:
    return Option.Some(x)

def worker(channel[int64] ch, int64 reps):
    auto acc = 0
    for i in range(0, reps):
        Pair[int64, str] p = make_pair(i, "item")
        Option[str] o = wrap(p.second)
        Option[int64] q = wrap(p.first)
        if o.is_some():
            acc = acc + q.unwrap()
    ch <- acc

def main():
    auto ch = channel[int64]()
    spawn worker(ch, 20000)
    spawn worker(ch, 20000)
    spawn worker(ch, 20000)
    spawn worker(ch, 20000)
    auto total = 0
    auto k = 0
    while k < 4:
        total = total + <-ch
        k = k + 1
    print(total)
