# Golden: the POINTER variant of Atomic[T] is GC-correct (I#176). `Atomic[Node]`
# is a one-field box whose cell is a reference, so monomorphization gives it a
# normal precise per-struct descriptor marking that word as a pointer — the
# atomic slot is a GC root scanned like any other, with no special collector code
# (reusing the existing ZapTypeDesc machinery, per the issue). After an atomic
# `swap`, we churn a lot of garbage and force a collection: the value reachable
# ONLY through the atomic cell must survive (a mis-scanned slot would free it, and
# the read-back would crash or print garbage). A pointer `compare_and_swap` then
# swings the cell by identity. Deterministic under both collectors.
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
    Atomic[Node] slot = Atomic(Node(v=1))
    Node n2 = Node(v=2)
    auto old = slot.swap(n2)
    churn(200000)
    gc.collect()
    auto cur = slot.load()
    print(old.v)
    print(cur.v)
    Node n3 = Node(v=3)
    print(b2i(slot.compare_and_swap(n2, n3)))
    churn(200000)
    gc.collect()
    print(slot.load().v)
