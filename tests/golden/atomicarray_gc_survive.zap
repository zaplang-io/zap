# AtomicArray[Node] reference slots are GC-correct under the moving collector
# (#844). Values reachable ONLY through the atomic-array slots must survive a
# collection — a mis-scanned or unforwarded slot would free/stale them and the
# read-back would crash or print garbage. One slot is left empty (null) to prove
# the trace skips null slots. After churning garbage and forcing a collection we
# read the survivors back; an identity compare_and_swap then swings a slot and a
# second collection confirms the new value survives too. Mirrors the Atomic[Node]
# pointer golden (atomic_pointer.zap). Deterministic under both collectors; no
# heap-byte assertions.
struct Node:
    int64 v

def b2i(bool x) -> int64:
    if x:
        return 1
    return 0

def churn(int64 n):
    for i in range(0, n):
        auto g = Node(v=i)

def main():
    AtomicArray[Node] a = AtomicArray[Node](3)
    a.store(0, Node(v=10))
    a.store(2, Node(v=30))  # slot 1 stays empty (null) — must not be scanned as a ptr
    churn(200000)
    gc.collect()
    print(a.load(0).v)  # 10  (survived via the slot only)
    print(a.load(2).v)  # 30
    auto keep = a.load(0)
    Node n99 = Node(v=99)
    print(b2i(a.compare_and_swap(0, keep, n99)))  # 1   (identity match -> swapped)
    churn(200000)
    gc.collect()
    print(a.load(0).v)  # 99  (new value survived)
    print(a.load(2).v)  # 30  (untouched, survived twice)
