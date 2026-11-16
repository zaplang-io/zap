# Module-qualified construction of the seam-exposed generic MPSC `Queue[T]`
# (I#996, D-0202). The natural dotted spellings
#   concurrent.Queue[int64] q = concurrent.Queue[int64]()   # explicit T
#   concurrent.Queue[int64] q = concurrent.Queue()          # T from annotation
# are pure front-end sugar for the bare prelude `new_queue[T]()`, which keeps
# working unchanged. Distinct element types monomorphize distinctly (they must
# not collapse), and the comma-ok pop / FIFO / drain-refill behavior is intact.
# Deterministic single-consumer; identical under both collectors.
import concurrent

def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def main():
    # Explicit type argument on the dotted constructor; comma-ok pop on empty,
    # FIFO order, then drain.
    concurrent.Queue[int64] q = concurrent.Queue[int64]()
    auto e0, ok0 = q.pop()
    print(b2i(ok0))  # 0  (empty)
    q.push(10)
    q.push(20)
    auto a, oka = q.pop()
    print(a.unwrap())  # 10
    print(b2i(oka))  # 1
    auto b, okb = q.pop()
    print(b.unwrap())  # 20
    auto e1, ok1 = q.pop()
    print(b2i(ok1))  # 0  (drained)

    # T inferred from the dotted annotation (arg-less dotted constructor).
    concurrent.Queue[int64] qi = concurrent.Queue()
    qi.push(7)
    auto c, okc = qi.pop()
    print(c.unwrap())  # 7

    # A reference (str) payload — a distinct instance, explicit dotted ctor.
    concurrent.Queue[str] qs = concurrent.Queue[str]()
    qs.push("alpha")
    qs.push("beta")
    auto s1, s1ok = qs.pop()
    print(s1.unwrap())  # alpha
    auto s2, s2ok = qs.pop()
    print(s2.unwrap())  # beta

    # The bare `new_queue[T]()` form still works and shares the int64 instance
    # with the dotted form above (back-compat, D-0202).
    Queue[int64] qb = new_queue()
    qb.push(99)
    auto d, okd = qb.pop()
    print(d.unwrap())  # 99

    # A bool payload — a third distinct monomorphization via the dotted form.
    concurrent.Queue[bool] qf = concurrent.Queue[bool]()
    qf.push(True)
    auto f, okf = qf.pop()
    print(b2i(f.unwrap()))  # 1
