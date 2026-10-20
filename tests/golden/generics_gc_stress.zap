# GC stress for monomorphized generics (I#154, D-0064). Four goroutines each
# churn many generic-typed heap instances with BOTH a pointer payload
# (Option[str], Pair[int64, str]) and an unboxed-scalar payload (Option[int64]),
# forcing collections mid-loop. Correct output under BOTH collectors proves each
# monomorphized instance gets a correct per-instance ZapTypeDesc pointer map from
# codegen's existing per-struct emission (the "GC metadata is free" theorem): a
# wrong map would mis-trace the str pointer or follow the i64 as a pointer and
# crash or corrupt. The summed result is order-independent, so it is
# deterministic regardless of scheduler interleaving.
struct Pair[A, B]:
    A first
    B second

def worker(channel[int64] ch, int64 reps):
    auto acc = 0
    for i in range(0, reps):
        Pair[int64, str] p = Pair(i, "item")
        Option[str] o = Option.Some(p.second)
        Option[int64] q = Option.Some(p.first)
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
